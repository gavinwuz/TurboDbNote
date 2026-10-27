//! Explicit dump adaptation for a NEW database. Original input is never modified.
use sqlparser::{
    dialect::MySqlDialect,
    tokenizer::{Token, Tokenizer},
};
use std::collections::BTreeMap;
pub use turbodbn_core::notebook::RestoreOptions as Options;
pub struct Preview {
    pub sql: String,
    pub source_databases: Vec<String>,
    pub tables: usize,
    pub duplicates: usize,
    pub removed_drops: usize,
    pub skipped_data: usize,
    pub skipped_triggers: usize,
    pub changed_definers: usize,
    pub generated_columns: bool,
}
/// SQL-aware splitter including mysql-client DELIMITER directives and compound triggers.
pub fn statements(sql: &str) -> Result<Vec<String>, String> {
    let b = sql.as_bytes();
    let (mut i, mut start) = (0, 0);
    let mut delimiter = b";".to_vec();
    let mut quote = None;
    let mut block = false;
    let mut line = false;
    let mut out = vec![];
    while i < b.len() {
        if line {
            if b[i] == b'\n' {
                line = false;
            }
            i += 1;
            continue;
        }
        if block {
            if b.get(i..i + 2) == Some(b"*/") {
                block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = quote {
            if b[i] == b'\\' {
                i = (i + 2).min(b.len());
                continue;
            }
            if b[i] == q {
                if b.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if (i == 0 || b[i - 1] == b'\n') && b[i..].starts_with(b"DELIMITER ") {
            let end = sql[i..].find('\n').map_or(b.len(), |n| i + n);
            let d = sql[i + 10..end].trim();
            if d.is_empty()
                || d.len() > 16
                || d.bytes()
                    .any(|c| c.is_ascii_whitespace() || c.is_ascii_alphanumeric())
            {
                return Err("restore.delimiter".into());
            }
            let prefix = super::workbench::script_context::expand(&sql[start..i], None)?;
            if !prefix.trim().is_empty() {
                return Err("restore.delimiter".into());
            }
            delimiter = d.as_bytes().to_vec();
            i = end;
            start = i;
            continue;
        }
        if b.get(i..i + 2) == Some(b"/*") {
            block = true;
            i += 2;
            continue;
        }
        if b[i] == b'#'
            || b.get(i..i + 2) == Some(b"--")
                && b.get(i + 2).is_none_or(|c| c.is_ascii_whitespace())
        {
            line = true;
            i += 1;
            continue;
        }
        if matches!(b[i], b'\'' | b'"' | b'`') {
            quote = Some(b[i]);
            i += 1;
            continue;
        }
        if b.get(i..i + delimiter.len()) == Some(delimiter.as_slice()) {
            out.push(sql[start..i].to_owned());
            i += delimiter.len();
            start = i;
            continue;
        }
        i += 1;
    }
    if quote.is_some() || block {
        return Err("script.error.unterminated".into());
    }
    if !sql[start..].trim().is_empty() {
        out.push(sql[start..].into());
    }
    Ok(out)
}
fn word(t: Option<&Token>) -> String {
    match t {
        Some(Token::Word(w)) if w.quote_style.is_none() => w.value.to_ascii_uppercase(),
        _ => String::new(),
    }
}
fn ident(t: Option<&Token>) -> Result<String, String> {
    match t {
        Some(Token::Word(w)) => Ok(w.value.clone()),
        _ => Err("restore.syntax".into()),
    }
}
fn ts(sql: &str) -> Result<Vec<Token>, String> {
    Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize()
        .map(|v| {
            v.into_iter()
                .filter(|t| !matches!(t, Token::Whitespace(_)))
                .collect()
        })
        .map_err(|_| "restore.syntax".into())
}
pub fn adapt(source: &str, options: &Options) -> Result<Preview, String> {
    if source.len() > 16 * 1024 * 1024
        || source.contains('\0')
        || options.database.is_empty()
        || options.database.len() > 64
        || options.database.chars().any(char::is_control)
    {
        return Err("restore.limit".into());
    }
    let mut preview = Preview {
        sql: String::new(),
        source_databases: vec![],
        tables: 0,
        duplicates: 0,
        removed_drops: 0,
        skipped_data: 0,
        skipped_triggers: 0,
        changed_definers: 0,
        generated_columns: false,
    };
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let parts = statements(source)?;
    let mut parsed = vec![];
    let mut database_options = String::new();
    for part in parts {
        let expanded = super::workbench::script_context::expand(&part, None)?;
        let tokens = ts(&expanded)?;
        if tokens.is_empty() {
            continue;
        }
        if word(tokens.first()) == "USE" {
            let db = ident(tokens.get(1))?;
            if !preview.source_databases.contains(&db) {
                preview.source_databases.push(db);
            }
        }
        if word(tokens.first()) == "CREATE" && word(tokens.get(1)) == "DATABASE" {
            let at = if word(tokens.get(2)) == "IF" { 5 } else { 2 };
            let db = ident(tokens.get(at))?;
            let suffix = tokens[at + 1..]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            if !database_options.is_empty() && database_options != suffix {
                return Err("restore.conflicting_duplicates".into());
            }
            database_options = suffix;

            if !preview.source_databases.contains(&db) {
                preview.source_databases.push(db);
            }
        }
        parsed.push((expanded, tokens));
    }
    if preview.source_databases.len() > 1 {
        return Err("restore.multiple_sources".into());
    }
    let target = super::workbench::quote(crate::DatabaseKind::MySql, &options.database);
    let views = parsed
        .iter()
        .filter_map(|(_, t)| {
            if word(t.first()) != "CREATE" {
                return None;
            }
            let at = t.iter().position(|t| word(Some(t)) == "VIEW")?;
            let mut at = at + 1;
            if word(t.get(at)) == "IF" {
                at += 3;
            }
            if t.get(at + 1) == Some(&Token::Period) {
                at += 2;
            }
            ident(t.get(at)).ok()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut adjusted_mode = false;
    let mut output = vec![
        format!("CREATE DATABASE {target} {database_options}"),
        format!("USE {target}"),
    ];
    let mut definitions = BTreeMap::new();
    for (sql, tokens) in parsed {
        let first = word(tokens.first());
        let second = word(tokens.get(1));
        if first == "USE" || first == "CREATE" && second == "DATABASE" {
            continue;
        }
        if first == "DROP" {
            if matches!(second.as_str(), "TABLE" | "VIEW" | "TRIGGER") {
                preview.removed_drops += 1;
                continue;
            }
            return Err("restore.statement".into());
        }
        if matches!(first.as_str(), "INSERT" | "REPLACE") && !options.data {
            preview.skipped_data += 1;
            continue;
        }
        let trigger =
            first == "CREATE" && tokens.iter().take(12).any(|t| word(Some(t)) == "TRIGGER");
        if trigger && !options.triggers {
            preview.skipped_triggers += 1;
            continue;
        }
        if first == "SET" {
            if tokens.contains(&Token::LParen) {
                return Err("restore.statement".into());
            }
            if tokens
                .iter()
                .any(|t| matches!(word(Some(t)).as_str(), "GLOBAL" | "PERSIST" | "PASSWORD"))
            {
                return Err("restore.statement".into());
            }
            if second != "NAMES" {
                for assignment in tokens[1..].split(|t| *t == Token::Comma) {
                    let Some(eq) = assignment.iter().position(|t| *t == Token::Eq) else {
                        return Err("restore.statement".into());
                    };
                    let key = assignment[..eq]
                        .iter()
                        .map(ToString::to_string)
                        .collect::<String>()
                        .to_ascii_uppercase();
                    let key = key
                        .trim_start_matches('@')
                        .trim_start_matches("OLD_")
                        .trim_start_matches("SAVED_");
                    if !matches!(
                        key,
                        "CHARACTER_SET_CLIENT"
                            | "CHARACTER_SET_RESULTS"
                            | "COLLATION_CONNECTION"
                            | "CS_CLIENT"
                            | "CS_RESULTS"
                            | "COL_CONNECTION"
                            | "FOREIGN_KEY_CHECKS"
                            | "UNIQUE_CHECKS"
                            | "SQL_MODE"
                            | "SQL_NOTES"
                            | "TIME_ZONE"
                    ) {
                        return Err("restore.statement".into());
                    }
                }
            }
        }
        if first == "LOCK" || first == "UNLOCK" {
            continue;
        }
        if !matches!(
            first.as_str(),
            "CREATE" | "INSERT" | "REPLACE" | "SET" | "ALTER"
        ) {
            return Err("restore.statement".into());
        }
        if first == "CREATE"
            && !matches!(
                second.as_str(),
                "TABLE" | "VIEW" | "DEFINER" | "ALGORITHM" | "SQL"
            )
        {
            return Err("restore.statement".into());
        }
        if first == "ALTER" {
            return Err("restore.statement".into());
        }
        if tokens.windows(2).any(|w| {
            matches!(
                (word(w.first()).as_str(), word(w.get(1)).as_str()),
                ("INTO", "OUTFILE") | ("LOAD", "DATA")
            )
        }) {
            return Err("restore.statement".into());
        }
        let locations = Tokenizer::new(&MySqlDialect {}, &sql)
            .tokenize_with_location()
            .map_err(|_| "restore.syntax")?
            .into_iter()
            .filter(|t| !matches!(t.token, Token::Whitespace(_)))
            .collect::<Vec<_>>();
        let offset = |line: u64, column: u64| -> Result<usize, String> {
            let (mut l, mut c) = (1, 1);
            for (i, ch) in sql.char_indices() {
                if (l, c) == (line, column) {
                    return Ok(i);
                }
                if ch == '\n' {
                    l += 1;
                    c = 1;
                } else {
                    c += 1;
                }
            }
            if (l, c) == (line, column) {
                Ok(sql.len())
            } else {
                Err("restore.syntax".into())
            }
        };
        for i in 0..tokens.len().saturating_sub(2) {
            if tokens.get(i + 1) == Some(&Token::Period)
                && matches!(tokens.get(i + 2), Some(Token::Word(_)))
            {
                let previous = i
                    .checked_sub(1)
                    .map(|j| word(tokens.get(j)))
                    .unwrap_or_default();
                if matches!(
                    previous.as_str(),
                    "TABLE"
                        | "VIEW"
                        | "TRIGGER"
                        | "INTO"
                        | "FROM"
                        | "JOIN"
                        | "REFERENCES"
                        | "EXISTS"
                ) || trigger && previous == "ON"
                {
                    let db = ident(tokens.get(i))?;
                    if !preview.source_databases.contains(&db) && db != options.database {
                        return Err("restore.cross_database".into());
                    }
                }
            }
        }
        let mut edits = vec![];
        for i in 0..locations.len() {
            if first == "SET"
                && tokens.iter().any(|t| word(Some(t)) == "SQL_MODE")
                && let Token::SingleQuotedString(value) = &locations[i].token
            {
                if value.split(',').any(|m| {
                    matches!(
                        m.trim().to_ascii_uppercase().as_str(),
                        "NO_BACKSLASH_ESCAPES" | "ANSI_QUOTES"
                    )
                }) {
                    return Err("script.error.mode".into());
                }
                if value
                    .split(',')
                    .any(|m| m.trim().eq_ignore_ascii_case("NO_AUTO_CREATE_USER"))
                {
                    let value = value
                        .split(',')
                        .filter(|m| !m.trim().eq_ignore_ascii_case("NO_AUTO_CREATE_USER"))
                        .collect::<Vec<_>>()
                        .join(",");
                    edits.push((
                        offset(locations[i].span.start.line, locations[i].span.start.column)?,
                        offset(locations[i].span.end.line, locations[i].span.end.column)?,
                        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")),
                    ));
                    adjusted_mode = true;
                }
            }

            if locations
                .get(i + 1)
                .is_some_and(|t| t.token == Token::Period)
                && matches!(&locations[i].token,Token::Word(w)if preview.source_databases.contains(&w.value))
            {
                // Qualified object references only. Three-part column references
                // are also database-qualified; alias.column is left alone.
                let previous = i
                    .checked_sub(1)
                    .map(|j| word(Some(&locations[j].token)))
                    .unwrap_or_default();
                let three = locations
                    .get(i + 3)
                    .is_some_and(|t| t.token == Token::Period);
                if three
                    || matches!(
                        previous.as_str(),
                        "TABLE"
                            | "VIEW"
                            | "TRIGGER"
                            | "FROM"
                            | "JOIN"
                            | "INTO"
                            | "UPDATE"
                            | "ON"
                            | "REFERENCES"
                            | "EXISTS"
                    )
                {
                    edits.push((
                        offset(locations[i].span.start.line, locations[i].span.start.column)?,
                        offset(locations[i].span.end.line, locations[i].span.end.column)?,
                        target.clone(),
                    ));
                }
            }
            if options.current_definer
                && word(Some(&locations[i].token)) == "DEFINER"
                && locations.get(i + 1).is_some_and(|t| t.token == Token::Eq)
            {
                let end = (i + 5).min(locations.len());
                if end != i + 5 {
                    return Err("restore.syntax".into());
                }
                edits.push((
                    offset(locations[i].span.start.line, locations[i].span.start.column)?,
                    offset(
                        locations[end - 1].span.end.line,
                        locations[end - 1].span.end.column,
                    )?,
                    String::new(),
                ));
                preview.changed_definers += 1;
            }
        }
        let mut adapted = sql.clone();
        for (a, b, value) in edits.into_iter().rev() {
            adapted.replace_range(a..b, &value);
        }
        if first == "CREATE" && second == "TABLE" {
            let at = if word(tokens.get(2)) == "IF" { 5 } else { 2 };
            let mut name = ident(tokens.get(at))?;
            if tokens.get(at + 1) == Some(&Token::Period) {
                name = ident(tokens.get(at + 2))?;
            }
            if views.contains(&name) {
                preview.duplicates += 1;
                continue;
            }
            let normalized = ts(&adapted)?
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(existing) = definitions.insert(name, normalized.clone()) {
                if existing != normalized {
                    return Err("restore.conflicting_duplicates".into());
                }
                preview.duplicates += 1;
                continue;
            }
            preview.tables += 1;
        }
        if first == "CREATE" && second != "TABLE" {
            let normalized = ts(&adapted)?
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            let at = tokens
                .iter()
                .position(|t| matches!(word(Some(t)).as_str(), "VIEW" | "TRIGGER"))
                .ok_or("restore.statement")?;
            let object = word(tokens.get(at));
            let mut at = at + 1;
            if tokens.get(at + 1) == Some(&Token::Period) {
                at += 2;
            }
            let name = format!("{object}:{}", ident(tokens.get(at))?);
            if let Some(previous) = definitions.insert(name, normalized.clone()) {
                if previous != normalized {
                    return Err("restore.conflicting_duplicates".into());
                }
                preview.duplicates += 1;
                continue;
            }
        }
        preview.generated_columns |= tokens.iter().any(|t| word(Some(t)) == "GENERATED");
        output.push(adapted);
    }
    // One client delimiter for all statements, including compound trigger bodies.
    let delimiter = "//$//";
    if output.iter().any(|s| s.contains(delimiter)) {
        return Err("restore.delimiter".into());
    }
    preview.sql = format!(
        "-- Adapted copy. Original source preserved. New database only.\nDELIMITER {delimiter}\n{}\nDELIMITER ;\n",
        output
            .iter()
            .map(|s| format!("{s}{delimiter}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if adjusted_mode {
        preview.sql.insert_str(0,"-- Compatibility adjustment: removed obsolete NO_AUTO_CREATE_USER from SQL_MODE for MySQL 8 compatibility.\n");
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> Options {
        Options {
            database: "local_copy".into(),
            data: false,
            triggers: true,
            current_definer: true,
        }
    }
    #[test]
    fn dump_mapping_preserves_literals_and_removes_duplicate_drop_cycles() {
        let source = "CREATE DATABASE /*!32312 IF NOT EXISTS*/ `online`; USE online; DROP TABLE IF EXISTS t; CREATE TABLE online.t(id INT COMMENT 'online.t'); DROP TABLE IF EXISTS t; CREATE TABLE online.t(id INT COMMENT 'online.t'); INSERT INTO online.t VALUES(1);";
        let p = adapt(source, &options()).unwrap();
        assert_eq!(p.tables, 1);
        assert_eq!(p.duplicates, 1);
        assert_eq!(p.removed_drops, 2);
        assert_eq!(p.skipped_data, 1);
        assert!(p.sql.contains("CREATE DATABASE `local_copy`"));
        assert!(p.sql.contains("CREATE TABLE `local_copy`.t"));
        assert!(p.sql.contains("'online.t'"));
        assert!(!p.sql.contains("DROP TABLE"));
        let p = adapt(
            "USE online; INSERT INTO t VALUES(1); INSERT INTO t VALUES(1);",
            &Options {
                data: true,
                ..options()
            },
        )
        .unwrap();
        assert_eq!(p.sql.matches("INSERT INTO").count(), 2);

        let p = adapt(
            "USE online; SET SQL_MODE='NO_AUTO_CREATE_USER,STRICT_TRANS_TABLES';",
            &options(),
        )
        .unwrap();
        assert!(p.sql.contains("SQL_MODE='STRICT_TRANS_TABLES'"));
        assert!(
            adapt(
                "USE online; SET SQL_MODE='NO_BACKSLASH_ESCAPES';",
                &options()
            )
            .is_err()
        );
    }
    #[test]
    fn delimiter_trigger_is_one_statement_and_definer_is_removed() {
        let source = "USE online;\nDELIMITER ;;\n/*!50003 CREATE*/ /*!50017 DEFINER=`account`@`host`*/ /*!50003 TRIGGER trg AFTER INSERT ON online.t FOR EACH ROW BEGIN SET @v='a;b'; INSERT INTO online.audit VALUES (1); END */;;\nDELIMITER ;\n";
        let p = adapt(source, &options()).unwrap();
        assert_eq!(p.changed_definers, 1);
        assert!(p.sql.contains("SET @v='a;b';"));
        assert!(!p.sql.contains("DEFINER"));
        assert!(p.sql.contains("ON `local_copy`.t"));
    }
    #[test]
    fn conflicts_and_multiple_sources_abort_without_a_partial_result() {
        assert!(
            adapt(
                "USE a; CREATE TABLE t(id INT); CREATE TABLE t(id TEXT);",
                &options()
            )
            .is_err()
        );
        assert!(adapt("USE a; USE b;", &options()).is_err());
        assert!(adapt("USE a; DROP DATABASE a;", &options()).is_err());
        assert!(adapt("USE a; SOURCE local.sql;", &options()).is_err());
    }
}
