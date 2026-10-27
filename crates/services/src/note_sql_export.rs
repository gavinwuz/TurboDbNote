//! Export a frozen Note in cell order, without executing or silently merging servers.
use crate::{DatabaseKind, mysql::Profile, sql_script};
use std::{collections::BTreeSet, path::Path, sync::atomic::AtomicBool};
use turbodbn_core::{CellContent, Note, notebook::SqlDialect};

const LIMIT: usize = 64 * 1024 * 1024;
pub struct Export {
    pub sql: String,
    pub cells: usize,
    pub skipped: usize,
    pub databases: Vec<String>,
    pub kind: DatabaseKind,
    pub connection: uuid::Uuid,
    pub connection_label: String,
    pub excluded: usize,
}
fn append(out: &mut String, value: &str) -> Result<(), String> {
    if out.len().saturating_add(value.len()) > LIMIT {
        return Err("notes.export_limit".into());
    }
    out.push_str(value);
    Ok(())
}
fn use_database(database: &str) -> Result<String, String> {
    if database.is_empty() || database.len() > 1024 || database.chars().any(char::is_control) {
        return Err("notes.export_scope".into());
    }
    Ok(format!("USE `{}`;\n", database.replace('`', "``")))
}
fn serialize(source: &str, dialect: SqlDialect) -> Result<String, String> {
    // Attachments may carry a UTF-8 BOM; it is invalid after generated headers/USE.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    if source.len() > LIMIT || source.contains('\0') {
        return Err("notes.export_limit".into());
    }
    if dialect == SqlDialect::MySql {
        let statements = crate::database::mysql_restore::statements(source)?;
        let mut out = String::new();
        for statement in statements {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            // Compound bodies keep their internal semicolons. A client delimiter cannot collide.
            if crate::database::mysql_restore::statements(statement)?.len() > 1 {
                let mut delimiter = "$$".to_owned();
                while statement.contains(&delimiter) {
                    delimiter.push('$');
                }
                append(
                    &mut out,
                    &format!("DELIMITER {delimiter}\n{statement}\n{delimiter}\nDELIMITER ;\n"),
                )?;
            } else {
                append(&mut out, statement)?;
                let trailing_comment = statement
                    .lines()
                    .last()
                    .is_some_and(|line| line.contains("--") || line.contains('#'));
                append(&mut out, if trailing_comment { "\n;\n" } else { ";\n" })?;
            }
        }
        Ok(out)
    } else {
        // Keep raw SQL, including dollar-quoted bodies and dialect-specific syntax.
        sql_script::validate_export_source(source, dialect)?;
        Ok(format!("{}\n;\n", source.trim()))
    }
}
pub fn connection_label(profile: &Profile) -> String {
    let address = if profile.kind == DatabaseKind::Sqlite {
        profile.sqlite_path.clone()
    } else {
        format!("{}:{}", profile.host, profile.port)
    };
    format!("{} · {}", profile.name, address)
}
/// Connections remain identities even when host/database names happen to match.
pub fn connections(note: &Note) -> Result<Vec<(uuid::Uuid, usize)>, String> {
    let mut choices: Vec<(uuid::Uuid, usize)> = Vec::new();
    for cell in &note.cells {
        let explicit = match &cell.content {
            CellContent::Sql {
                source,
                connection_id,
                ..
            }
            | CellContent::SqlScriptText {
                source,
                connection_id,
                ..
            } => {
                if source.trim().is_empty() {
                    continue;
                }
                *connection_id
            }
            CellContent::SqlScript { connection_id, .. } => *connection_id,
            _ => continue,
        };
        let id = explicit
            .or(note.default_connection_id)
            .ok_or("notes.export_scope")?;
        if let Some((_, count)) = choices.iter_mut().find(|(connection, _)| *connection == id) {
            *count += 1;
        } else {
            choices.push((id, 1));
        }
    }
    Ok(choices)
}
/// Track top-level USE without rewriting any authored SQL. Ambiguous execution restores scope.
fn final_database(source: &str, dialect: SqlDialect, initial: &str) -> Option<String> {
    use sqlparser::{
        dialect::{ClickHouseDialect, Dialect, MySqlDialect},
        tokenizer::{Token, Tokenizer},
    };
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let statements = if dialect == SqlDialect::MySql {
        crate::database::mysql_restore::statements(source).ok()?
    } else {
        sql_script::editor_ranges(source, dialect)
            .ok()?
            .into_iter()
            .map(|range| source[range].to_owned())
            .collect()
    };
    let mut current = Some(initial.to_owned());
    for statement in statements {
        // Conditional executable comments may or may not run on the importing server.
        if statement.contains("/*!") || statement.contains("/*M!") || statement.len() > 1024 * 1024
        {
            current = None;
            continue;
        }
        let dialect: &dyn Dialect = if dialect == SqlDialect::ClickHouse {
            &ClickHouseDialect {}
        } else {
            &MySqlDialect {}
        };
        let Ok(tokens) = Tokenizer::new(dialect, &statement).tokenize() else {
            current = None;
            continue;
        };
        let mut tokens = tokens
            .into_iter()
            .filter(|token| !matches!(token, Token::Whitespace(_)))
            .collect::<Vec<_>>();
        while tokens.last() == Some(&Token::SemiColon) {
            tokens.pop();
        }
        let Some(Token::Word(first)) = tokens.first() else {
            if !tokens.is_empty() {
                current = None;
            }
            continue;
        };
        if first.quote_style.is_some() {
            current = None;
            continue;
        }
        match first.value.to_ascii_uppercase().as_str() {
            "USE" => current=match tokens.as_slice() {
                [_,Token::Word(database)]=>Some(database.value.clone()),
                _=>None,
            },
            "CALL" | "EXECUTE" | "PREPARE" | "SET" => current=None,
            _ => {
                if tokens.iter().any(|token|matches!(token,Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case("USE"))) {current=None;}
            }
        }
    }
    current
}
pub fn prepare(note: &Note, path: &Path, profiles: &[Profile]) -> Result<Export, String> {
    prepare_inner(note, path, profiles, None)
}
pub fn prepare_selected(
    note: &Note,
    path: &Path,
    profiles: &[Profile],
    connection: uuid::Uuid,
) -> Result<Export, String> {
    prepare_inner(note, path, profiles, Some(connection))
}
fn prepare_inner(
    note: &Note,
    path: &Path,
    profiles: &[Profile],
    selected: Option<uuid::Uuid>,
) -> Result<Export, String> {
    let mut export = Export {
        sql: "-- Exported by TurboDBNote. Review before executing.\n".into(),
        cells: 0,
        skipped: 0,
        databases: vec![],
        kind: DatabaseKind::MySql,
        connection: uuid::Uuid::nil(),
        connection_label: String::new(),
        excluded: 0,
    };
    let mut connection = None;
    let mut databases = BTreeSet::new();
    let mut active_database: Option<String> = None;
    for cell in &note.cells {
        let (id, database, source, attachment) = match &cell.content {
            CellContent::Sql {
                source,
                connection_id,
                database,
            }
            | CellContent::SqlScriptText {
                source,
                connection_id,
                database,
            } => (*connection_id, database, Some(source.as_str()), None),
            CellContent::SqlScript {
                script,
                connection_id,
                database,
            } => (*connection_id, database, None, Some(script)),
            _ => {
                export.skipped += 1;
                continue;
            }
        };
        if source.is_some_and(|s| s.trim().is_empty()) {
            continue;
        }
        let inherits = id.is_none();
        let id = id
            .or(note.default_connection_id)
            .ok_or("notes.export_scope")?;
        if selected.is_some_and(|selected| selected != id) {
            export.excluded += 1;
            continue;
        }
        let profile = profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or("notes.export_scope")?;
        if connection.is_some_and(|current| current != id) {
            return Err("notes.export_connections".into());
        }
        connection = Some(id);
        export.kind = profile.kind;
        export.connection = id;
        let dialect = sql_script::dialect(profile.kind).ok_or("notes.export_dialect")?;
        let database = if profile.kind == DatabaseKind::Sqlite {
            profile.sqlite_path.as_str()
        } else {
            database
                .as_deref()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    inherits
                        .then_some(note.default_database.as_deref())
                        .flatten()
                        .filter(|v| !v.trim().is_empty())
                })
                .unwrap_or(&profile.database)
        };
        if database.is_empty() {
            return Err("notes.export_scope".into());
        }
        databases.insert(database.to_owned());
        if !matches!(profile.kind, DatabaseKind::MySql | DatabaseKind::ClickHouse)
            && databases.len() > 1
        {
            return Err("notes.export_databases".into());
        }
        let source = if let Some(script) = attachment {
            if script.bytes > LIMIT as u64 {
                return Err("notes.export_limit".into());
            }
            if script.dialect != dialect {
                return Err("notes.export_dialect".into());
            }
            if script.restore.is_some() {
                return Err("notes.export_restore".into());
            }
            sql_script::freeze(path, script, &AtomicBool::new(false))?
                .export_source(LIMIT as u64)?
        } else {
            source.unwrap().to_owned()
        };
        let sql = serialize(&source, dialect)?;
        if sql.trim().is_empty() {
            continue;
        }
        if export.cells == 0 {
            export.connection_label = connection_label(profile);
            append(
                &mut export.sql,
                &format!(
                    "-- Connection: {}\n",
                    export.connection_label.replace(char::is_control, " ")
                ),
            )?;
        }
        append(
            &mut export.sql,
            &format!("\n-- SQL cell {}\n", export.cells + 1),
        )?;
        // Omit only generated duplicates; preserve authored USE and restore after internal switches.
        if matches!(profile.kind, DatabaseKind::MySql | DatabaseKind::ClickHouse) {
            if active_database.as_deref() != Some(database) {
                append(&mut export.sql, &use_database(database)?)?;
            }
            active_database = final_database(&source, dialect, database);
        }
        append(&mut export.sql, &sql)?;
        export.cells += 1;
    }
    if export.cells == 0 {
        return Err("notes.export_empty".into());
    }
    export.databases = databases.into_iter().collect();
    Ok(export)
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbodbn_core::Cell;
    fn fixture() -> (Note, Profile) {
        let profile = Profile::default();
        let mut note = Note::new("Export");
        note.default_connection_id = Some(profile.id);
        note.default_database = Some("first".into());
        (note, profile)
    }
    fn cell(source: &str, database: Option<&str>) -> Cell {
        Cell {
            id: uuid::Uuid::new_v4(),
            title: String::new(),
            content: CellContent::Sql {
                source: source.into(),
                connection_id: None,
                database: database.map(str::to_owned),
            },
        }
    }
    #[test]
    fn cell_scopes_restore_after_authored_use_and_escape_names() {
        let (mut note, profile) = fixture();
        note.cells = vec![
            cell("USE other; SELECT 'a;b';", None),
            cell("SELECT 2 -- end", Some("sec`ond")),
            cell("SELECT 3", None),
        ];
        let out = prepare(&note, Path::new("unused.note"), &[profile]).unwrap();
        assert_eq!(out.cells, 3);
        assert_eq!(out.sql.matches("USE `first`;").count(), 2);
        assert!(out.sql.contains("USE `sec``ond`;"));
        let parsed = crate::database::mysql_restore::statements(&out.sql).unwrap();
        assert_eq!(parsed.iter().filter(|s| s.contains("SELECT")).count(), 3);
    }
    #[test]
    fn consecutive_same_scope_only_generates_one_use() {
        let (mut note, profile) = fixture();
        note.cells = vec![
            cell("ALTER TABLE a ADD INDEX i (id);", None),
            cell("SELECT 'USE other;'; -- USE other", None),
            cell("SELECT 3;", None),
        ];
        let export = prepare(&note, Path::new("unused.note"), &[profile]).unwrap();
        assert_eq!(export.sql.matches("USE `first`;").count(), 1);
    }
    #[test]
    fn internal_use_switches_and_conditional_context_are_not_lost() {
        let (mut note, profile) = fixture();
        note.cells = vec![
            cell("USE other; SELECT 1;", None),
            cell("SELECT 2;", None),
            cell("SELECT 3;", None),
        ];
        let export = prepare(
            &note,
            Path::new("unused.note"),
            std::slice::from_ref(&profile),
        )
        .unwrap();
        assert_eq!(export.sql.matches("USE `first`;").count(), 2);
        assert!(export.sql.contains("USE other;"));
        note.cells = vec![
            cell("/*!80000 USE other */;", None),
            cell("SELECT 2;", None),
        ];
        assert_eq!(
            prepare(&note, Path::new("unused.note"), &[profile])
                .unwrap()
                .sql
                .matches("USE `first`;")
                .count(),
            2
        );
    }
    #[test]
    fn selected_connection_excludes_other_servers_even_for_same_database() {
        let (mut note, profile) = fixture();
        let other = Profile {
            name: "Other server".into(),
            host: "192.0.2.10".into(),
            ..Profile::default()
        };
        let mut foreign = cell("SELECT 'foreign';", Some("first"));
        if let CellContent::Sql { connection_id, .. } = &mut foreign.content {
            *connection_id = Some(other.id);
        }
        note.cells = vec![
            cell("SELECT 'mine';", None),
            foreign,
            cell("SELECT 'mine again';", None),
        ];
        assert_eq!(
            connections(&note).unwrap(),
            vec![(profile.id, 2), (other.id, 1)]
        );
        let profiles = vec![profile.clone(), other.clone()];
        assert!(prepare(&note, Path::new("unused.note"), &profiles).is_err());
        let mine =
            prepare_selected(&note, Path::new("unused.note"), &profiles, profile.id).unwrap();
        assert_eq!((mine.cells, mine.excluded), (2, 1));
        assert!(!mine.sql.contains("foreign"));
        assert_eq!(mine.sql.matches("USE `first`;").count(), 1);
        let theirs =
            prepare_selected(&note, Path::new("unused.note"), &profiles, other.id).unwrap();
        assert_eq!((theirs.cells, theirs.excluded), (1, 2));
        assert!(!theirs.sql.contains("mine"));
        assert!(theirs.sql.contains("Other server"));
    }
    #[test]
    fn clickhouse_scopes_restore_after_quoted_use() {
        let (mut note, mut profile) = fixture();
        profile.kind = DatabaseKind::ClickHouse;
        note.cells = vec![
            cell("SELECT 1;", None),
            cell("USE \"other\"; SELECT 2;", None),
            cell("SELECT 3;", None),
        ];
        let export = prepare(&note, Path::new("unused.note"), &[profile]).unwrap();
        assert_eq!(export.sql.matches("USE `first`;").count(), 2);
    }
    #[test]
    fn explicit_connection_uses_its_database_not_note_default() {
        let (mut note, mut profile) = fixture();
        profile.database = "profiledb".into();
        let mut explicit = cell("SELECT 2", None);
        if let CellContent::Sql { connection_id, .. } = &mut explicit.content {
            *connection_id = Some(profile.id);
        }
        note.cells = vec![cell("SELECT 1", None), explicit];
        let export = prepare(&note, Path::new("unused.note"), &[profile]).unwrap();
        assert!(export.sql.contains("USE `first`;"));
        assert!(export.sql.contains("USE `profiledb`;"));
    }
    #[test]
    fn attached_sql_is_verified_and_included_in_order() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("export.note");
        let cancel = AtomicBool::new(false);
        let prepared =
            sql_script::prepare_text("\u{feff}SELECT 'attached';", SqlDialect::MySql, &cancel)
                .unwrap();
        prepared.publish(&path, &cancel).unwrap();
        let (mut note, profile) = fixture();
        note.cells = vec![
            cell("SELECT 'first';", None),
            Cell::new(
                "file",
                CellContent::SqlScript {
                    script: prepared.script.clone(),
                    connection_id: None,
                    database: Some("other".into()),
                },
            ),
        ];
        let export = prepare(&note, &path, std::slice::from_ref(&profile)).unwrap();
        assert!(export.sql.find("'first'").unwrap() < export.sql.find("'attached'").unwrap());
        assert!(export.sql.contains("USE `other`;"));
        assert!(!export.sql.contains('\u{feff}'));
        let source = root
            .path()
            .join(".turbodbnote-scripts")
            .join(format!("{}.sql", prepared.script.source_hash));
        std::fs::write(source, "SELECT 'tampered';").unwrap();
        assert!(prepare(&note, &path, &[profile]).is_err());
    }
    #[test]
    fn non_mysql_files_can_exceed_editor_buffer_limit() {
        let source = "SELECT 1;\n".repeat(110_000);
        assert!(serialize(&source, SqlDialect::Sqlite).is_ok());
        assert!(serialize("SELECT 'unfinished", SqlDialect::Sqlite).is_err());
    }
    #[test]
    fn compound_body_and_delimiter_round_trip() {
        let source = "DELIMITER $$\nCREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN SET @x='a;b'; SET @y=2; END$$\nDELIMITER ;\nSELECT 1;";
        let out = serialize(source, SqlDialect::MySql).unwrap();
        assert_eq!(
            crate::database::mysql_restore::statements(source)
                .unwrap()
                .iter()
                .map(|s| s.trim())
                .collect::<Vec<_>>(),
            crate::database::mysql_restore::statements(&out)
                .unwrap()
                .iter()
                .map(|s| s.trim())
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn refuses_cross_connection_and_non_switchable_databases() {
        let (mut note, profile) = fixture();
        let other = Profile::default();
        let mut second = cell("SELECT 2", None);
        if let CellContent::Sql { connection_id, .. } = &mut second.content {
            *connection_id = Some(other.id);
        }
        note.cells = vec![cell("SELECT 1", None), second];
        assert_eq!(
            prepare(&note, Path::new("unused.note"), &[profile.clone(), other])
                .err()
                .unwrap(),
            "notes.export_connections"
        );
        let mut profile = profile;
        profile.kind = DatabaseKind::PostgreSql;
        note.cells = vec![cell("SELECT 1", None), cell("SELECT 2", Some("second"))];
        assert_eq!(
            prepare(&note, Path::new("unused.note"), &[profile])
                .err()
                .unwrap(),
            "notes.export_databases"
        );
    }
}
