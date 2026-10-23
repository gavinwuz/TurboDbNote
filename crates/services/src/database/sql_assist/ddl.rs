//! Read-only editor targeting. Tolerant tokens never enter execution validation.
use super::*;
use sqlparser::tokenizer::Location;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdlTable {
    pub table: TableName,
    pub line: usize,
}
#[derive(Default, Debug)]
pub struct DdlTarget {
    pub tables: Vec<DdlTable>,
    pub selected: Option<TableName>,
    pub fallback: Vec<DdlTable>,
    pub recognized: bool,
}
struct Located {
    token: Token,
    range: Range<usize>,
    line: usize,
}
fn tokens(kind: Option<DatabaseKind>, source: &str) -> Vec<Located> {
    let mut tokens = vec![];
    let _ =
        Tokenizer::new(dialect(kind).as_ref(), source).tokenize_with_location_into_buf(&mut tokens);
    let mut chars = source.char_indices();
    let (mut line, mut col, mut byte) = (1, 1, 0);
    let mut offset = |loc: Location| {
        while (line, col) < (loc.line, loc.column) {
            let Some((i, c)) = chars.next() else {
                return source.len();
            };
            byte = i + c.len_utf8();
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        byte
    };
    tokens
        .into_iter()
        .map(|t| {
            let start = offset(t.span.start);
            let end = offset(t.span.end);
            Located {
                token: t.token,
                range: start..end,
                line: t.span.start.line as usize,
            }
        })
        .collect()
}
fn keyword(token: &Token, name: &str) -> bool {
    matches!(token,Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(name))
}
fn identifier(kind: Option<DatabaseKind>, token: &Token) -> Option<String> {
    let Token::Word(word) = token else {
        return None;
    };
    if word.value.is_empty() || word.value.len() > 256 || word.value.chars().any(char::is_control) {
        return None;
    }
    Some(
        if kind == Some(DatabaseKind::PostgreSql) && word.quote_style.is_none() {
            word.value.to_lowercase()
        } else {
            word.value.clone()
        },
    )
}
fn name(
    kind: Option<DatabaseKind>,
    tokens: &[&Located],
    i: usize,
) -> Option<(TableName, Range<usize>)> {
    let first = tokens.get(i)?;
    let mut table = TableName {
        schema: String::new(),
        name: identifier(kind, &first.token)?,
    };
    let mut end = first.range.end;
    if tokens
        .get(i + 1)
        .is_some_and(|t| matches!(t.token, Token::Period))
    {
        let last = tokens.get(i + 2)?;
        table.schema = table.name;
        table.name = identifier(kind, &last.token)?;
        end = last.range.end;
        if tokens
            .get(i + 3)
            .is_some_and(|t| matches!(t.token, Token::Period))
        {
            return None;
        }
    }
    Some((table, first.range.start..end))
}
fn ranges(source: &str, tokens: &[Located]) -> Vec<Range<usize>> {
    let mut start = 0;
    let mut ranges = vec![];
    for token in tokens {
        if matches!(token.token, Token::SemiColon) {
            ranges.push(start..token.range.end);
            start = token.range.end;
        }
    }
    if start < source.len() {
        ranges.push(start..source.len());
    }
    ranges
}
fn add(tables: &mut Vec<DdlTable>, table: DdlTable) {
    if tables.len() < 32 && !tables.iter().any(|old| old.table == table.table) {
        tables.push(table);
    }
}
fn relations(
    kind: Option<DatabaseKind>,
    source: &str,
    all: &[Located],
    range: &Range<usize>,
) -> Vec<(DdlTable, Range<usize>)> {
    let ts = all
        .iter()
        .filter(|t| {
            range.start <= t.range.start
                && t.range.end <= range.end
                && !matches!(t.token, Token::Whitespace(_))
        })
        .collect::<Vec<_>>();
    let mut ctes = vec![];
    if ts.first().is_some_and(|t| keyword(&t.token, "WITH")) {
        for i in 1..ts.len().saturating_sub(1) {
            if keyword(&ts[i].token, "AS") && matches!(ts[i + 1].token, Token::LParen) {
                let mut name = i - 1;
                if matches!(ts[name].token, Token::RParen) {
                    let mut depth = 1usize;
                    while name > 0 && depth > 0 {
                        name -= 1;
                        match ts[name].token {
                            Token::RParen => depth += 1,
                            Token::LParen => depth -= 1,
                            _ => {}
                        }
                    }
                    name = name.saturating_sub(1);
                }
                if let Some(cte) = identifier(kind, &ts[name].token) {
                    ctes.push(cte);
                }
            }
        }
    }
    // AST-based source list remains authoritative where parsing succeeds.
    let sql = &source[range.clone()];
    let expected = if sql.len() <= MAX_SOURCE {
        super::columns::column_sources(kind, sql, sql.len())
    } else {
        vec![]
    };
    let mut result = vec![];
    let mut nesting = vec![];
    let mut expression_depth = 0usize;
    let mut from = false;
    for i in 0..ts.len() {
        match ts[i].token {
            Token::LParen => {
                let query = ts
                    .get(i + 1)
                    .is_some_and(|t| keyword(&t.token, "SELECT") || keyword(&t.token, "WITH"));
                nesting.push(expression_depth);
                expression_depth = if query { 0 } else { expression_depth + 1 };
            }
            Token::RParen => expression_depth = nesting.pop().unwrap_or(0),
            _ => {}
        }
        if ["WHERE", "GROUP", "ORDER", "HAVING", "SET", "RETURNING"]
            .iter()
            .any(|k| keyword(&ts[i].token, k))
        {
            from = false;
        }
        let comma = from && expression_depth == 0 && matches!(ts[i].token, Token::Comma);
        if keyword(&ts[i].token, "FROM") {
            from = true;
        }
        let relation = comma
            || ["FROM", "JOIN", "UPDATE", "INTO", "TABLE"]
                .iter()
                .any(|k| keyword(&ts[i].token, k))
            || i == 0
                && ["DESCRIBE", "DESC"]
                    .iter()
                    .any(|k| keyword(&ts[i].token, k));
        if !relation {
            continue;
        }
        let mut next = i + 1;
        if ts
            .get(next)
            .is_some_and(|t| keyword(&t.token, "ONLY") || keyword(&t.token, "LATERAL"))
        {
            next += 1;
        }
        let Some((table, span)) = name(kind, &ts, next) else {
            continue;
        };
        if table.schema.is_empty() && ctes.iter().any(|cte| cte.eq_ignore_ascii_case(&table.name)) {
            continue;
        }
        let is_expected = expected.iter().any(|t| {
            t.name.eq_ignore_ascii_case(&table.name) && t.schema.eq_ignore_ascii_case(&table.schema)
        });
        if !expected.is_empty() && !is_expected {
            continue;
        }
        if expression_depth > 0 && !is_expected {
            continue;
        }
        if ["FROM", "JOIN"].iter().any(|k| keyword(&ts[i].token, k))
            && ts
                .iter()
                .find(|t| t.range.start >= span.end)
                .is_some_and(|t| matches!(t.token, Token::LParen))
        {
            continue;
        }
        result.push((
            DdlTable {
                table,
                line: ts[next].line,
            },
            span,
        ));
    }
    // Includes comma joins and physical sources inside CTEs/derived queries.
    for table in expected {
        if result.iter().any(|(t, _)| t.table == table) {
            continue;
        }
        if let Some(t) = ts
            .iter()
            .find(|t| identifier(kind, &t.token).as_deref() == Some(table.name.as_str()))
        {
            result.push((
                DdlTable {
                    table,
                    line: t.line,
                },
                0..0,
            ));
        }
    }
    result
}
fn unique(refs: &[(DdlTable, Range<usize>)]) -> Vec<DdlTable> {
    let mut result = vec![];
    for (table, _) in refs {
        add(&mut result, table.clone());
    }
    result
}
fn collect(
    kind: Option<DatabaseKind>,
    source: &str,
    all: &[Located],
    rs: &[Range<usize>],
) -> Vec<DdlTable> {
    let mut result = vec![];
    for range in rs.iter().take(128) {
        for (table, _) in relations(kind, source, all, range) {
            add(&mut result, table);
        }
    }
    result
}
fn caret_column(
    kind: Option<DatabaseKind>,
    source: &str,
    all: &[Located],
    range: &Range<usize>,
    cursor: usize,
) -> Option<TableName> {
    if range.len() > MAX_SOURCE {
        return None;
    }
    let sql = &source[range.clone()];
    let local = cursor.saturating_sub(range.start).min(sql.len());
    if let Some(request) =
        super::columns::column_request(kind, sql, local).filter(|r| r.local_columns.is_none())
    {
        return Some(request.table);
    }
    let tokens = all
        .iter()
        .filter(|t| {
            range.start <= t.range.start
                && t.range.end <= range.end
                && !matches!(t.token, Token::Whitespace(_))
        })
        .collect::<Vec<_>>();
    for group in tokens.windows(3) {
        if matches!(group[0].token, Token::Word(_))
            && matches!(group[1].token, Token::Period)
            && matches!(group[2].token, Token::Word(_))
            && group[0].range.start <= cursor
            && cursor <= group[2].range.end
        {
            let mut repaired = sql.to_owned();
            repaired.replace_range(
                group[2].range.start - range.start..group[2].range.end - range.start,
                "__ddl_probe__",
            );
            if let Some(request) =
                super::columns::column_request(kind, &repaired, group[1].range.end - range.start)
                    .filter(|r| r.local_columns.is_none())
            {
                return Some(request.table);
            }
        }
    }
    None
}
/// Selection identifiers must occupy a relation token, not a matching literal/column/comment.
pub fn ddl_target(
    kind: Option<DatabaseKind>,
    source: &str,
    mut selection: Range<usize>,
    cursor: usize,
) -> DdlTarget {
    if source.len() > turbodbn_core::notebook::MAX_SCRIPT_TEXT_BYTES
        || selection.start > selection.end
        || selection.end > source.len()
        || cursor > source.len()
        || !source.is_char_boundary(selection.start)
        || !source.is_char_boundary(selection.end)
        || !source.is_char_boundary(cursor)
        || kind == Some(DatabaseKind::Redis)
    {
        return DdlTarget::default();
    }
    if !selection.is_empty() {
        let raw = &source[selection.clone()];
        let left = raw.len() - raw.trim_start().len();
        let right = raw.trim_end().len();
        selection = if left >= right {
            cursor..cursor
        } else {
            selection.start + left..selection.start + right
        };
    }
    let all = tokens(kind, source);
    let rs = ranges(source, &all);
    let anchor = if selection.is_empty() {
        cursor
    } else {
        selection.start
    };
    let current = rs
        .iter()
        .find(|r| r.start <= anchor && anchor < r.end)
        .or_else(|| rs.last());
    let fallback = || collect(kind, source, &all, &rs);
    if !selection.is_empty() {
        let blocked = all.iter().any(|t| {
            t.range.start <= selection.start
                && selection.end <= t.range.end
                && !matches!(t.token, Token::Word(_) | Token::Period)
        });
        if !blocked {
            let selected = source[selection.clone()].trim();
            let selected_tokens = tokens(kind, selected);
            let first = selected_tokens
                .iter()
                .find(|t| !matches!(t.token, Token::Whitespace(_)));
            let sql_selection = first.is_some_and(|t| {
                [
                    "SELECT", "WITH", "UPDATE", "DELETE", "INSERT", "ALTER", "CREATE", "DROP",
                    "TRUNCATE", "EXPLAIN", "DESCRIBE",
                ]
                .iter()
                .any(|k| keyword(&t.token, k))
            });
            if sql_selection && selected.len() <= MAX_SOURCE {
                let selected_ranges = ranges(selected, &selected_tokens);
                let line_offset = source[..selection.start]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count();
                let mut tables = collect(kind, selected, &selected_tokens, &selected_ranges);
                // Leading spaces have no line effect; leading newlines do.
                let leading =
                    source[selection.clone()].len() - source[selection.clone()].trim_start().len();
                let leading_lines = source[selection.start..selection.start + leading]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count();
                for table in &mut tables {
                    table.line += line_offset + leading_lines;
                }
                let chosen = (tables.len() == 1).then(|| tables[0].table.clone());
                let recognized = !tables.is_empty();
                return DdlTarget {
                    tables,
                    selected: chosen,
                    fallback: fallback(),
                    recognized,
                };
            }
            if let Some(range) = current {
                let refs = relations(kind, source, &all, range);
                if let Some((table, _)) = refs.iter().find(|(table, span)| {
                    span.start <= selection.start && selection.end <= span.end && {
                        let raw = &source[selection.clone()];
                        let leaf = all
                            .iter()
                            .find(|t| t.range.end == span.end && matches!(t.token, Token::Word(_)));
                        raw == &source[span.clone()]
                            || leaf.is_some_and(|leaf| {
                                leaf.range.start <= selection.start
                                    && selection.end <= leaf.range.end
                                    && {
                                        let token = &source[leaf.range.clone()];
                                        let inner =
                                            if token.starts_with('`') || token.starts_with('"') {
                                                &token[1..token.len() - 1]
                                            } else {
                                                token
                                            };
                                        raw == inner
                                            || raw == table.table.name
                                            || kind == Some(DatabaseKind::PostgreSql)
                                                && raw.to_lowercase() == table.table.name
                                    }
                            })
                    }
                }) {
                    return DdlTarget {
                        tables: unique(&refs),
                        selected: Some(table.table.clone()),
                        fallback: fallback(),
                        recognized: true,
                    };
                }
            }
        }
        return DdlTarget {
            fallback: fallback(),
            ..Default::default()
        };
    }
    if let Some(range) = current {
        let refs = relations(kind, source, &all, range);
        let tables = unique(&refs);
        let preferred = refs
            .iter()
            .find(|(_, span)| span.start <= cursor && cursor <= span.end)
            .map(|(t, _)| t.table.clone())
            .or_else(|| caret_column(kind, source, &all, range, cursor))
            .or_else(|| (tables.len() == 1).then(|| tables[0].table.clone()));
        return DdlTarget {
            recognized: !tables.is_empty(),
            tables,
            selected: preferred,
            fallback: fallback(),
        };
    }
    DdlTarget {
        fallback: fallback(),
        ..Default::default()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn picked(sql: &str, text: &str) -> DdlTarget {
        let start = sql.rfind(text).unwrap();
        ddl_target(Some(DatabaseKind::MySql), sql, start..start + text.len(), 0)
    }
    #[test]
    fn selected_relation_and_statement_override_caret() {
        let sql = "SELECT * FROM first_table; SELECT * FROM `client_update_log`; SELECT * FROM last_table;";
        assert_eq!(
            picked(sql, "client_update_log").selected.unwrap().name,
            "client_update_log"
        );
        assert_eq!(
            picked(sql, "`client_update_log`").selected.unwrap().name,
            "client_update_log"
        );
        assert_eq!(
            picked(sql, "SELECT * FROM `client_update_log`;")
                .selected
                .unwrap()
                .name,
            "client_update_log"
        );
        let multiple = picked(
            sql,
            "SELECT * FROM first_table; SELECT * FROM `client_update_log`;",
        );
        assert!(multiple.selected.is_none());
        assert_eq!(multiple.tables.len(), 2);
    }
    #[test]
    fn quoted_qualified_names_and_incomplete_neighbours() {
        let sql = "SELECT * FROM `other-db`.`odd``table`; SELECT 'unterminated";
        assert_eq!(
            picked(sql, "`other-db`.`odd``table`").selected.unwrap(),
            TableName {
                schema: "other-db".into(),
                name: "odd`table".into()
            }
        );
        let sql = "SELECT 'unfinished\nSELECT * FROM real_table;";
        assert_eq!(
            picked(sql, "SELECT * FROM real_table;")
                .selected
                .unwrap()
                .name,
            "real_table"
        );
        let sql = "SELECT * FROM t WHERE x = 'unfinished";
        assert_eq!(picked(sql, "t").selected.unwrap().name, "t");
    }
    #[test]
    fn literals_comments_columns_and_ctes_are_not_relations() {
        for (sql, word) in [
            ("SELECT 'users' FROM users", "users"),
            ("SELECT id FROM users WHERE id=1", "id"),
            ("SELECT * FROM users; -- users", "users"),
            ("WITH v AS (SELECT * FROM real_table) SELECT * FROM v", "v"),
        ] {
            let start = if sql.starts_with("SELECT 'users'") {
                8
            } else {
                sql.rfind(word).unwrap()
            };
            let target = ddl_target(
                Some(DatabaseKind::MySql),
                sql,
                start..start + word.len(),
                start,
            );
            assert!(target.selected.is_none(), "{sql}");
            assert!(!target.recognized);
        }
    }
    #[test]
    fn statement_lexer_keeps_literals_comments_and_self_joins_separate() {
        let sql = "SELECT 'a;b' FROM users u JOIN users other ON u.id=other.id; -- ; comment\nSELECT * FROM teams;";
        assert_eq!(picked(sql, "users other").selected, None);
        let selected = picked(
            sql,
            "SELECT 'a;b' FROM users u JOIN users other ON u.id=other.id;",
        );
        assert_eq!(selected.tables.len(), 1);
        assert_eq!(selected.selected.unwrap().name, "users");
        let sql = "SELECT * FROM users, teams WHERE 1=1;";
        assert_eq!(picked(sql, "teams").selected.unwrap().name, "teams");
        let sql = "SELECT * FROM  `users`  ;";
        assert_eq!(picked(sql, "  `users`  ").selected.unwrap().name, "users");
    }
    #[test]
    fn caret_alias_and_quoted_columns_resolve_in_the_current_scope() {
        let sql = "SELECT u.`id`, t.name FROM users u JOIN teams t ON u.`id`=t.id;";
        for offset in [sql.find("u.`id`").unwrap(), sql.find("`id`").unwrap() + 2] {
            assert_eq!(
                ddl_target(Some(DatabaseKind::MySql), sql, offset..offset, offset)
                    .selected
                    .unwrap()
                    .name,
                "users"
            );
        }
    }
    #[test]
    fn long_units_and_unicode_selection_keep_positions() {
        let sql = format!("-- 😀 {}\nSELECT * FROM target;", "x".repeat(65_000));
        let result = picked(&sql, "target");
        assert_eq!(result.selected.unwrap().name, "target");
        assert_eq!(result.tables[0].line, 2);
        let sql = "SELECT * FROM a;\nSELECT * FROM b;";
        let offset = sql.find("SELECT * FROM b").unwrap();
        assert_eq!(
            ddl_target(Some(DatabaseKind::MySql), sql, offset..offset, offset)
                .selected
                .unwrap()
                .name,
            "b"
        );
    }
}
