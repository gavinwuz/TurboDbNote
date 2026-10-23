//! Offline, bounded SQL assistance. Never opens connections or executes generated text.
use crate::DatabaseKind;
use sqlparser::{
    dialect::{
        ClickHouseDialect, Dialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
    },
    parser::Parser,
    tokenizer::{Token, Tokenizer, Whitespace},
};
use std::ops::Range;

mod catalog;
mod columns;
mod context;
mod ddl;
pub use columns::{ColumnRequest, column_request, column_sources, column_suggestions, ddl_tables};
pub use ddl::{DdlTable, DdlTarget, ddl_target};
pub const MAX_SOURCE: usize = 60_000;
pub const MAX_SUGGESTIONS: usize = 60;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    Unsupported,
    TooLarge,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableName {
    pub schema: String,
    pub name: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Keyword,
    Function,
    Table,
    Query,
    Command,
    Column,
}
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub label: String,
    pub insert: String,
    pub signature: String,
    pub kind: SuggestionKind,
    pub range: Range<usize>,
}

fn dialect(kind: Option<DatabaseKind>) -> Box<dyn Dialect> {
    match kind {
        Some(DatabaseKind::MySql) => Box::new(MySqlDialect {}),
        Some(DatabaseKind::PostgreSql) => Box::new(PostgreSqlDialect {}),
        Some(DatabaseKind::Sqlite) => Box::new(SQLiteDialect {}),
        Some(DatabaseKind::ClickHouse) => Box::new(ClickHouseDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}
fn word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$')
}
fn quote(kind: Option<DatabaseKind>, value: &str) -> String {
    let q = if matches!(kind, Some(DatabaseKind::MySql | DatabaseKind::ClickHouse)) {
        '`'
    } else {
        '"'
    };
    format!("{q}{}{q}", value.replace(q, &format!("{q}{q}")))
}
fn qualified(kind: Option<DatabaseKind>, table: &TableName) -> String {
    if table.schema.is_empty() {
        quote(kind, &table.name)
    } else {
        format!(
            "{}.{}",
            quote(kind, &table.schema),
            quote(kind, &table.name)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionWord {
    pub range: Range<usize>,
    pub prefix: String,
    pub schema: Option<String>,
}
#[derive(Default)]
pub struct WordMemo {
    last: Option<(Option<DatabaseKind>, String, Option<Option<String>>)>,
}
impl WordMemo {
    pub fn request(
        &mut self,
        kind: Option<DatabaseKind>,
        source: &str,
        offset: usize,
    ) -> Option<CompletionWord> {
        if source.len() > MAX_SOURCE || offset > source.len() || !source.is_char_boundary(offset) {
            return None;
        }
        let start = source[..offset]
            .char_indices()
            .rev()
            .find(|(_, c)| !word(*c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let end = source[offset..]
            .char_indices()
            .find(|(_, c)| !word(*c))
            .map_or(source.len(), |(i, _)| offset + i);
        let head = &source[..start];
        let schema = if let Some((old_kind, old_head, value)) = &self.last
            && *old_kind == kind
            && old_head == head
        {
            value.clone()
        } else {
            let value = (|| {
                let tokens = Tokenizer::new(dialect(kind).as_ref(), head)
                    .tokenize()
                    .ok()?;
                if tokens.last().is_some_and(|token|matches!(token,Token::Whitespace(Whitespace::SingleLineComment{comment,..}) if !comment.ends_with('\n'))){return None;}
                let mut tokens = tokens
                    .iter()
                    .rev()
                    .filter(|token| !matches!(token, Token::Whitespace(_)));
                let schema = if matches!(tokens.next(), Some(Token::Period)) {
                    if let Some(Token::Word(name)) = tokens.next() {
                        Some(name.value.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                Some(schema)
            })();
            self.last = Some((kind, head.into(), value.clone()));
            value
        }?;
        Some(CompletionWord {
            range: start..end,
            prefix: source[start..offset].to_lowercase(),
            schema,
        })
    }
}
pub fn completion_word(
    kind: Option<DatabaseKind>,
    source: &str,
    offset: usize,
) -> Option<CompletionWord> {
    WordMemo::default().request(kind, source, offset)
}
/// SELECT insertion is limited to a new statement containing only this table prefix.
/// A newline alone never terminates a statement; comments and quoted semicolons are lexical tokens.
pub fn table_select_allowed(
    kind: Option<DatabaseKind>,
    source: &str,
    word: &CompletionWord,
) -> bool {
    if source.len() > MAX_SOURCE
        || word.schema.is_some()
        || word.range.end > source.len()
        || !source.is_char_boundary(word.range.start)
        || !source.is_char_boundary(word.range.end)
    {
        return false;
    }
    let Ok(head) = Tokenizer::new(dialect(kind).as_ref(), &source[..word.range.start]).tokenize()
    else {
        return false;
    };
    let head = head
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let mut depth = 0usize;
    for token in &head {
        match token {
            Token::LParen => depth += 1,
            Token::RParen if depth == 0 => return false,
            Token::RParen => depth -= 1,
            _ => {}
        }
    }
    if depth != 0 {
        return false;
    }
    if head.last().is_some_and(|t| !matches!(t, Token::SemiColon)) {
        return false;
    }
    let Ok(tail) = Tokenizer::new(dialect(kind).as_ref(), &source[word.range.end..]).tokenize()
    else {
        return false;
    };
    let tail = tail
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    tail.is_empty() || matches!(tail.as_slice(), [Token::SemiColon])
}

pub fn table_select_text(
    kind: Option<DatabaseKind>,
    source: &str,
    word: &CompletionWord,
    table: &str,
) -> Option<String> {
    if !table_select_allowed(kind, source, word) {
        return None;
    }
    let tail = Tokenizer::new(dialect(kind).as_ref(), &source[word.range.end..])
        .tokenize()
        .ok()?;
    Some(format!(
        "SELECT * FROM {table}{}",
        if tail.iter().any(|t| matches!(t, Token::SemiColon)) {
            ""
        } else {
            ";"
        }
    ))
}

pub fn table_suggestions(
    kind: Option<DatabaseKind>,
    word: &CompletionWord,
    tables: &[TableName],
    limit: usize,
) -> Vec<Suggestion> {
    let mut tables = tables
        .iter()
        .filter(|table| {
            table.name.to_lowercase().starts_with(&word.prefix)
                && word
                    .schema
                    .as_ref()
                    .is_none_or(|schema| table.schema.eq_ignore_ascii_case(schema))
        })
        .collect::<Vec<_>>();
    tables.sort_by_key(|table| {
        (
            table.name.to_lowercase() != word.prefix,
            table.name.to_lowercase(),
            table.schema.to_lowercase(),
        )
    });
    tables.dedup_by(|a, b| a.schema == b.schema && a.name == b.name);
    tables
        .into_iter()
        .take(limit.min(MAX_SUGGESTIONS))
        .map(|table| Suggestion {
            label: if table.schema.is_empty() {
                table.name.clone()
            } else {
                format!("{}.{}", table.schema, table.name)
            },
            insert: if word.schema.is_some() {
                quote(kind, &table.name)
            } else {
                qualified(kind, table)
            },
            signature: String::new(),
            kind: SuggestionKind::Table,
            range: word.range.clone(),
        })
        .collect()
}
/// Reuse a resolved alias only while text outside the active identifier is byte-for-byte unchanged.
#[derive(Default)]
pub struct ColumnMemo {
    last: Option<(Option<DatabaseKind>, String, String, Option<ColumnRequest>)>,
}
impl ColumnMemo {
    pub fn request(
        &mut self,
        kind: Option<DatabaseKind>,
        source: &str,
        offset: usize,
    ) -> Option<ColumnRequest> {
        let word = completion_word(kind, source, offset)?;
        self.request_word(kind, source, offset, &word)
    }
    pub fn request_word(
        &mut self,
        kind: Option<DatabaseKind>,
        source: &str,
        offset: usize,
        word: &CompletionWord,
    ) -> Option<ColumnRequest> {
        let head = &source[..word.range.start];
        let tail = &source[word.range.end..];
        if let Some((old_kind, old_head, old_tail, request)) = &self.last
            && *old_kind == kind
            && old_head == head
            && old_tail == tail
        {
            return request.clone().map(|mut request| {
                request.prefix = word.prefix.clone();
                request.range = word.range.clone();
                request
            });
        }
        let request = column_request(kind, source, offset);
        self.last = Some((kind, head.into(), tail.into(), request.clone()));
        request
    }
}

/// Replace only the word at the caret; strings and comments never receive suggestions.
pub fn complete(
    kind: Option<DatabaseKind>,
    source: &str,
    offset: usize,
    tables: &[TableName],
) -> Vec<Suggestion> {
    if source.len() > MAX_SOURCE || offset > source.len() || !source.is_char_boundary(offset) {
        return vec![];
    }
    let start = source[..offset]
        .char_indices()
        .rev()
        .find(|(_, c)| !word(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = source[offset..]
        .char_indices()
        .find(|(_, c)| !word(*c))
        .map_or(source.len(), |(i, _)| offset + i);
    let prefix = &source[start..offset];
    let before = &source[..start];
    let Ok(tokens) = Tokenizer::new(dialect(kind).as_ref(), before).tokenize() else {
        return vec![];
    };
    if tokens.last().is_some_and(|t| matches!(t, Token::Whitespace(Whitespace::SingleLineComment { comment, .. }) if !comment.ends_with('\n'))) { return vec![]; }
    let significant = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let last = significant
        .last()
        .map(|t| t.to_string().to_ascii_uppercase())
        .unwrap_or_default();
    let statement_start = significant.is_empty() || last == ";";
    let remainder = &source[end..];
    let template_allowed =
        statement_start && (remainder.trim().is_empty() || remainder.trim_start().starts_with(';'));
    let template_end = if remainder.trim_start().starts_with(';') {
        end + remainder.len() - remainder.trim_start().len() + 1
    } else {
        end
    };
    let schema = if last == "." {
        significant.iter().rev().nth(1).and_then(|t| {
            if let Token::Word(w) = t {
                Some(w.value.as_str())
            } else {
                None
            }
        })
    } else {
        None
    };
    let relation = matches!(last.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE");
    if prefix.is_empty() && !relation {
        return vec![];
    }
    let mut items = Vec::new();
    let mut push = |label: String, insert: String, signature: String, item_kind| {
        if items.len() >= MAX_SUGGESTIONS {
            return;
        }
        items.push(Suggestion {
            label,
            insert,
            signature,
            kind: item_kind,
            range: start..if item_kind == SuggestionKind::Query {
                template_end
            } else {
                end
            },
        });
    };
    let matching = |s: &str| s.to_lowercase().starts_with(&prefix.to_lowercase());
    if kind == Some(DatabaseKind::Redis) {
        if statement_start {
            for &(name, signature) in catalog::REDIS {
                if matching(name) {
                    push(
                        name.into(),
                        format!("{name} "),
                        signature.into(),
                        SuggestionKind::Command,
                    );
                }
            }
        }
        items.truncate(MAX_SUGGESTIONS);
        return items;
    }
    let mut matched_table = false;
    if statement_start || relation || last == "." {
        for table in tables.iter().take(2000) {
            if !matching(&table.name)
                || table.name.len() > 256
                || table.schema.len() > 256
                || schema.is_some_and(|s| s != table.schema)
            {
                continue;
            }
            let name = if last == "." {
                quote(kind, &table.name)
            } else {
                qualified(kind, table)
            };
            let label = if table.schema.is_empty() {
                table.name.clone()
            } else {
                format!("{}.{}", table.schema, table.name)
            };
            if template_allowed {
                push(
                    label,
                    format!("SELECT * FROM {name};"),
                    name,
                    SuggestionKind::Query,
                );
            } else {
                push(label, name, String::new(), SuggestionKind::Table);
            }
            matched_table = true;
        }
    }
    if !relation && last != "." {
        for name in catalog::keywords(kind) {
            if matching(name) {
                push(
                    name.into(),
                    name.into(),
                    String::new(),
                    SuggestionKind::Keyword,
                );
            }
        }
        for (name, signature) in catalog::functions(kind) {
            if matching(name) {
                push(
                    name.into(),
                    format!("{name}()"),
                    signature.into(),
                    SuggestionKind::Function,
                );
            }
        }
        if template_allowed
            && !matched_table
            && prefix.chars().count() >= 2
            && prefix.len() <= 256
            && !catalog::keywords(kind)
                .iter()
                .any(|k| k.eq_ignore_ascii_case(prefix))
            && !catalog::functions(kind)
                .iter()
                .any(|(f, _)| f.eq_ignore_ascii_case(prefix))
        {
            let name = quote(kind, &source[start..end]);
            push(
                source[start..end].into(),
                format!("SELECT * FROM {name};"),
                name,
                SuggestionKind::Query,
            );
        }
    }
    items.truncate(MAX_SUGGESTIONS);
    items
}

// Kept as raw slices: tokenizer Display may normalize escapes and quote styles.
struct Piece<'a> {
    token: Token,
    raw: &'a str,
    start: usize,
    end: usize,
}
fn pieces(kind: Option<DatabaseKind>, sql: &str) -> Result<Vec<Piece<'_>>, FormatError> {
    let tokens = Tokenizer::new(dialect(kind).as_ref(), sql)
        .tokenize_with_location()
        .map_err(|_| FormatError::Invalid)?;
    let mut positions = std::collections::HashMap::new();
    let (mut line, mut column) = (1u64, 1u64);
    for (i, c) in sql.char_indices() {
        positions.insert((line, column), i);
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    positions.insert((line, column), sql.len());
    let mut result = Vec::new();
    for t in tokens {
        if matches!(
            t.token,
            Token::Whitespace(Whitespace::Space | Whitespace::Tab | Whitespace::Newline)
        ) {
            continue;
        }
        let start = *positions
            .get(&(t.span.start.line, t.span.start.column))
            .ok_or(FormatError::Invalid)?;
        let end = *positions
            .get(&(t.span.end.line, t.span.end.column))
            .ok_or(FormatError::Invalid)?;
        result.push(Piece {
            token: t.token,
            raw: &sql[start..end],
            start,
            end,
        });
    }
    Ok(result)
}

/// Whitespace-only formatting with a soft width, token identity and AST checks.
/// Unsupported/incomplete syntax is returned as an error without changing the document.
pub fn format(kind: Option<DatabaseKind>, sql: &str) -> Result<String, FormatError> {
    if kind == Some(DatabaseKind::Redis) {
        return Err(FormatError::Unsupported);
    }
    if sql.len() > MAX_SOURCE {
        return Err(FormatError::TooLarge);
    }
    if sql.trim().is_empty() {
        return Ok(sql.into());
    }
    if sql.contains("/*!") || sql.contains("/*M!") {
        return Err(FormatError::Invalid);
    }
    let d = dialect(kind);
    let parse = |text: &str| {
        Parser::new(d.as_ref())
            .with_recursion_limit(64)
            .try_with_sql(text)
            .and_then(|mut p| p.parse_statements())
            .map(|s| s.iter().map(ToString::to_string).collect::<Vec<_>>())
            .map_err(|_| FormatError::Invalid)
    };
    let original = parse(sql)?;
    let input = pieces(kind, sql)?;
    let mut out = String::new();
    let mut depth = 0usize;
    let mut subqueries = Vec::new();
    for (i, p) in input.iter().enumerate() {
        let prev = i.checked_sub(1).map(|i| &input[i]);
        let upper = match &p.token {
            Token::Word(w) if w.quote_style.is_none() => w.value.to_ascii_uppercase(),
            _ => String::new(),
        };
        let closing_query = matches!(p.token, Token::RParen) && subqueries.last() == Some(&true);
        if matches!(p.token, Token::RParen) {
            depth = depth.saturating_sub(1);
            subqueries.pop();
        }
        let query_level = depth == 0 || subqueries.last() == Some(&true);
        let clause = query_level
            && matches!(
                upper.as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "HAVING"
                    | "LIMIT"
                    | "OFFSET"
                    | "UNION"
                    | "RETURNING"
                    | "SET"
                    | "VALUES"
                    | "JOIN"
                    | "LEFT"
                    | "RIGHT"
                    | "INNER"
                    | "CROSS"
                    | "FULL"
                    | "ORDER"
                    | "GROUP"
            );
        let clause = clause && !(upper == "JOIN" && prev.is_some_and(|p| matches!(&p.token, Token::Word(w) if matches!(w.value.to_ascii_uppercase().as_str(), "LEFT" | "RIGHT" | "INNER" | "CROSS" | "FULL" | "OUTER"))));
        let comma = query_level && prev.is_some_and(|p| matches!(p.token, Token::Comma));
        // PostgreSQL concatenates adjacent literals only across a newline. Keep original
        // line boundaries as well as tokens; parser ASTs alone cannot prove this property.
        let line_boundary = prev.is_some_and(|last| sql[last.end..p.start].contains('\n'));
        let separator =
            prev.is_some_and(|p| matches!(p.token, Token::SemiColon | Token::Whitespace(_)));
        let tight = prev.is_some_and(|last| {
            last.end == p.start
                || matches!(
                    last.token,
                    Token::LParen | Token::Period | Token::DoubleColon
                )
        }) || matches!(
            p.token,
            Token::RParen | Token::Comma | Token::SemiColon | Token::Period | Token::DoubleColon
        );
        let width =
            out.rsplit('\n').next().unwrap_or("").chars().count() + p.raw.chars().count() + 1;
        let wrap = width > 100 && (!tight || prev.is_some_and(|p| matches!(p.token, Token::Comma)));
        if !out.is_empty() {
            if clause || comma || separator || wrap || line_boundary || closing_query {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&"  ".repeat(depth.min(16) + usize::from(comma || wrap)));
            } else if !tight {
                out.push(' ');
            }
        }
        out.push_str(p.raw);
        if matches!(p.token, Token::LParen) {
            depth += 1;
            subqueries.push(input.get(i+1).is_some_and(|p| matches!(&p.token, Token::Word(w) if w.value.eq_ignore_ascii_case("SELECT") || w.value.eq_ignore_ascii_case("WITH"))));
        }
    }
    let after = pieces(kind, &out)?;
    if input.iter().map(|p| p.raw).ne(after.iter().map(|p| p.raw)) || original != parse(&out)? {
        return Err(FormatError::Invalid);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn table(name: &str) -> TableName {
        TableName {
            schema: String::new(),
            name: name.into(),
        }
    }
    #[test]
    fn t_select_generation_respects_statements_comments_and_relation_positions() {
        for (source, allowed) in [
            ("", true),
            ("ga", true),
            ("SELECT 1;\nga", true),
            ("-- previous;\nga", true),
            ("SELECT 1\nga", false),
            ("SELECT * FROM ga", false),
            ("SELECT * FROM a JOIN ga", false),
            ("UPDATE ga", false),
            ("SELECT (1; ga", false),
            ("INSERT INTO ga", false),
            ("SELECT ';'\nga", false),
            ("SELECT ';';\nga", true),
            ("ga; SELECT 2", false),
        ] {
            let word = completion_word(
                Some(DatabaseKind::MySql),
                source,
                if source == "ga; SELECT 2" {
                    2
                } else {
                    source.len()
                },
            )
            .unwrap();
            assert_eq!(
                table_select_allowed(Some(DatabaseKind::MySql), source, &word),
                allowed,
                "{source}"
            );
        }
        let source = "SELECT 1;\nga /* keep comment */;";
        let word = completion_word(
            Some(DatabaseKind::MySql),
            source,
            source.find("ga").unwrap() + 2,
        )
        .unwrap();
        assert_eq!(
            table_select_text(Some(DatabaseKind::MySql), source, &word, "`game_account`"),
            Some("SELECT * FROM `game_account`".into())
        );
        assert_eq!(&source[word.range.clone()], "ga");
    }
    #[test]
    fn mysql_hash_and_dash_comments_survive_formatting_and_suppress_completion() {
        let sql = "-- 查询说明\n  # 不执行 DELETE FROM items;\nselect '#文字', '--文字' from items # 结尾注释";
        let result = format(Some(DatabaseKind::MySql), sql).unwrap();
        for text in [
            "-- 查询说明",
            "# 不执行 DELETE FROM items;",
            "'#文字'",
            "'--文字'",
            "# 结尾注释",
        ] {
            assert!(result.contains(text));
        }
        for sql in ["-- SEL", "  # SEL", "SELECT 1 # SEL"] {
            assert!(complete(Some(DatabaseKind::MySql), sql, sql.len(), &[]).is_empty());
        }
    }
    #[test]
    fn table_templates_use_dialect_quotes_and_replace_only_current_word() {
        let sql = "-- note\n用尾";
        let result = complete(
            Some(DatabaseKind::MySql),
            sql,
            "-- note\n用".len(),
            &[table("用户`表")],
        );
        let item = result
            .iter()
            .find(|s| s.kind == SuggestionKind::Query)
            .unwrap();
        assert_eq!(item.insert, "SELECT * FROM `用户``表`;");
        assert_eq!(item.range, "-- note\n".len()..sql.len());
        let tables = vec![TableName {
            schema: "public".into(),
            name: "orders".into(),
        }];
        let result = complete(
            Some(DatabaseKind::PostgreSql),
            "SELECT * FROM ord",
            17,
            &tables,
        );
        assert_eq!(result[0].insert, "\"public\".\"orders\"");
        assert_eq!(result[0].kind, SuggestionKind::Table);
        let result = complete(Some(DatabaseKind::Sqlite), "orders", 6, &[]);
        assert!(
            result
                .iter()
                .any(|s| s.insert == "SELECT * FROM \"orders\";")
        );
    }
    #[test]
    fn existing_semicolon_schema_and_surrounding_sql_are_preserved() {
        let source = "orders; SELECT 2;";
        let candidate = complete(Some(DatabaseKind::MySql), source, 6, &[])
            .into_iter()
            .find(|s| s.kind == SuggestionKind::Query)
            .unwrap();
        let mut changed = source.to_owned();
        changed.replace_range(candidate.range, &candidate.insert);
        assert_eq!(changed, "SELECT * FROM `orders`; SELECT 2;");
        assert!(
            !complete(None, "orders WHERE id=1", 6, &[])
                .iter()
                .any(|s| s.kind == SuggestionKind::Query)
        );
        let tables = vec![
            TableName {
                schema: "first".into(),
                name: "orders".into(),
            },
            TableName {
                schema: "second".into(),
                name: "orders".into(),
            },
        ];
        let candidates = complete(
            Some(DatabaseKind::PostgreSql),
            "SELECT * FROM first.ord",
            23,
            &tables,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].label, "first.orders");
    }
    #[test]
    fn formatted_width_is_soft_and_long_literals_are_untouched() {
        let literal = "x".repeat(150);
        let sql = format!(
            "SELECT first_column, second_column, third_column, '{literal}' FROM orders WHERE first_column = 1;"
        );
        let result = format(Some(DatabaseKind::MySql), &sql).unwrap();
        assert!(result.contains(&format!("'{literal}'")));
        assert!(
            result
                .lines()
                .filter(|l| !l.contains(&literal))
                .all(|l| l.chars().count() <= 100)
        );
        assert!(
            format(
                Some(DatabaseKind::PostgreSql),
                "SELECT E'a\\n', $tag$hi\nthere$tag$;\r\n-- hello\r\nSELECT 2;"
            )
            .is_ok()
        );
    }
    #[test]
    fn dialect_catalogs_and_redis_commands_do_not_mix() {
        for (kind, function) in [
            (DatabaseKind::MySql, "DATE_FORMAT"),
            (DatabaseKind::PostgreSql, "DATE_TRUNC"),
            (DatabaseKind::Sqlite, "STRFTIME"),
            (DatabaseKind::ClickHouse, "uniqExact"),
        ] {
            let result = complete(Some(kind), function, function.len(), &[]);
            assert!(result.iter().any(|s| s.kind == SuggestionKind::Function
                && s.label == function
                && !s.signature.is_empty()));
        }
        assert!(
            !complete(Some(DatabaseKind::Sqlite), "DATE_F", 6, &[])
                .iter()
                .any(|s| s.kind == SuggestionKind::Function)
        );
        let redis = complete(Some(DatabaseKind::Redis), "HG", 2, &[]);
        assert!(redis.iter().all(|s| s.kind == SuggestionKind::Command));
        assert!(redis.iter().any(|s| s.signature == "HGET key field"));
        assert!(complete(Some(DatabaseKind::Redis), "GET sec", 7, &[]).is_empty());
    }
    #[test]
    fn comments_literals_and_invalid_offsets_never_offer_completions() {
        for text in [
            "SELECT 'sec",
            "SELECT $$secret",
            "-- secret",
            "SELECT /* secret",
            "SELECT 1 # secret",
        ] {
            let kind = if text.contains("$$") {
                DatabaseKind::PostgreSql
            } else {
                DatabaseKind::MySql
            };
            assert!(
                complete(Some(kind), text, text.len(), &[]).is_empty(),
                "{text}"
            );
        }
        assert!(complete(None, "中文", 1, &[]).is_empty());
        assert!(complete(None, &"a".repeat(MAX_SOURCE + 1), MAX_SOURCE + 1, &[]).is_empty());
        let tables = (0..2000)
            .map(|i| table(&format!("orders_{i}")))
            .collect::<Vec<_>>();
        assert_eq!(complete(None, "orders", 6, &tables).len(), MAX_SUGGESTIONS);
    }
    #[test]
    fn formatting_preserves_tokens_literals_comments_and_is_idempotent() {
        for (kind, sql) in [
            (
                DatabaseKind::MySql,
                "select a,b,IFNULL(c, 'x  y') from `t` where a=1 and b='-- 注释' order by a;",
            ),
            (
                DatabaseKind::PostgreSql,
                "SELECT 'a  b', $$line\ntext$$, x::text FROM public.t WHERE x = $1; -- keep\nSELECT 2;",
            ),
            (
                DatabaseKind::Sqlite,
                "SELECT\t[中文], 'a''b' /* keep */ FROM t WHERE x = ?;",
            ),
            (
                DatabaseKind::ClickHouse,
                "SELECT toDate(ts), count() FROM events GROUP BY toDate(ts) ORDER BY toDate(ts);",
            ),
        ] {
            let formatted = format(Some(kind), sql).unwrap();
            assert!(formatted.contains('\n'));
            assert_eq!(
                pieces(Some(kind), sql)
                    .unwrap()
                    .iter()
                    .map(|p| p.raw)
                    .collect::<Vec<_>>(),
                pieces(Some(kind), &formatted)
                    .unwrap()
                    .iter()
                    .map(|p| p.raw)
                    .collect::<Vec<_>>()
            );
            assert_eq!(format(Some(kind), &formatted).unwrap(), formatted);
        }
    }
    #[test]
    fn newline_sensitive_literals_and_nested_queries_keep_boundaries() {
        let source = "SELECT 'first'\n'second';";
        if let Ok(result) = format(Some(DatabaseKind::PostgreSql), source) {
            assert!(result.contains("'first'\n'second'"));
        }
        let source = "SELECT q.id FROM (SELECT id, name FROM items WHERE id > 1) AS q LEFT JOIN teams ON q.id=teams.id;";
        let result = format(Some(DatabaseKind::PostgreSql), source).unwrap();
        assert!(result.contains("LEFT JOIN"));
        assert!(result.contains("\n  SELECT"));
        assert_eq!(
            format(Some(DatabaseKind::PostgreSql), &result).unwrap(),
            result
        );
    }
    #[test]
    fn unsupported_or_unsafe_formatting_preserves_source() {
        assert_eq!(
            format(Some(DatabaseKind::Redis), "GET key"),
            Err(FormatError::Unsupported)
        );
        for sql in [
            "SELECT 'unterminated",
            "SELECT (",
            "/*!40101 SET NAMES utf8 */;",
        ] {
            assert!(format(Some(DatabaseKind::MySql), sql).is_err());
        }
        assert_eq!(
            format(None, &"a".repeat(MAX_SOURCE + 1)),
            Err(FormatError::TooLarge)
        );
    }
}

#[cfg(test)]
mod cached_completion_tests {
    use super::*;
    #[test]
    fn table_top_k_rechecks_the_full_prefix_and_only_replaces_the_identifier() {
        let tables = (0..200)
            .map(|n| TableName {
                schema: String::new(),
                name: format!("cq_{n:03}"),
            })
            .chain(std::iter::once(TableName {
                schema: String::new(),
                name: "cq_itemtype".into(),
            }))
            .collect::<Vec<_>>();
        let first = completion_word(Some(DatabaseKind::MySql), "SELECT * FROM c", 15).unwrap();
        assert_eq!(
            table_suggestions(Some(DatabaseKind::MySql), &first, &tables, 8).len(),
            8
        );
        let sql = "SELECT * FROM cq_it WHERE id=1";
        let at = sql.find(" WHERE").unwrap();
        let word = completion_word(Some(DatabaseKind::MySql), sql, at).unwrap();
        let found = table_suggestions(Some(DatabaseKind::MySql), &word, &tables, 8);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].insert, "`cq_itemtype`");
        assert_eq!(&sql[found[0].range.clone()], "cq_it");
        assert!(completion_word(Some(DatabaseKind::MySql), "-- comment cq", 13).is_none());
        assert!(completion_word(Some(DatabaseKind::MySql), "SELECT 'cq", 10).is_none());
    }
    #[test]
    fn memo_reuses_alias_resolution_only_when_surrounding_sql_is_unchanged() {
        let mut memo = ColumnMemo::default();
        let source = "SELECT u. FROM users u";
        let first = memo.request(Some(DatabaseKind::MySql), source, 9).unwrap();
        assert_eq!(first.table.name, "users");
        let source = "SELECT u.n FROM users u";
        let next = memo.request(Some(DatabaseKind::MySql), source, 10).unwrap();
        assert_eq!(next.table.name, "users");
        assert_eq!(next.prefix, "n");
        assert_eq!(next.range, 9..10);
        let source = "SELECT u.n FROM accounts u";
        assert_eq!(
            memo.request(Some(DatabaseKind::MySql), source, 10)
                .unwrap()
                .table
                .name,
            "accounts"
        );
        let source = "WITH u AS (SELECT * FROM users) SELECT u.n FROM u";
        assert_eq!(
            memo.request(
                Some(DatabaseKind::MySql),
                source,
                source.find(".n").unwrap() + 2
            )
            .unwrap()
            .table
            .name,
            "users"
        );
    }
}
