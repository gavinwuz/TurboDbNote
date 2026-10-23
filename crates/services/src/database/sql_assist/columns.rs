use super::*;
use sqlparser::ast::{Expr, Ident, Query, SetExpr, TableFactor, Visit, Visitor};
use std::ops::ControlFlow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnRequest {
    pub table: TableName,
    pub prefix: String,
    pub range: Range<usize>,
    pub local_columns: Option<Vec<String>>,
}

/// Parse a temporary identifier at the caret, never execute the repaired text.
/// Derived tables/CTEs and ambiguous bindings are deliberately not guessed.
fn statement_range(
    kind: Option<DatabaseKind>,
    source: &str,
    offset: usize,
) -> Option<Range<usize>> {
    if source.len() > MAX_SOURCE || !source.is_char_boundary(offset) {
        return None;
    }
    let dialect = match kind {
        Some(DatabaseKind::MySql | DatabaseKind::ClickHouse) => {
            turbodbn_core::notebook::SqlDialect::MySql
        }
        Some(DatabaseKind::Sqlite) => turbodbn_core::notebook::SqlDialect::Sqlite,
        Some(DatabaseKind::Redis) => return None,
        _ => turbodbn_core::notebook::SqlDialect::PostgreSql,
    };
    crate::sql_script::editor_ranges(source, dialect)
        .ok()?
        .into_iter()
        .find(|range| range.start <= offset && offset <= range.end)
}
pub fn column_request(
    kind: Option<DatabaseKind>,
    source: &str,
    offset: usize,
) -> Option<ColumnRequest> {
    let range = statement_range(kind, source, offset)?;
    let sql = &source[range.clone()];
    let cursor = offset - range.start;
    let mut request = column_request_inner(kind, sql, cursor)
        .or_else(|| super::context::column_request(kind, sql, cursor))?;
    request.range.start += range.start;
    request.range.end += range.start;
    Some(request)
}
/// Explicit Col fallback: offer only actual relations referenced by this statement.
pub fn column_sources(kind: Option<DatabaseKind>, source: &str, offset: usize) -> Vec<TableName> {
    let Some(range) = statement_range(kind, source, offset) else {
        return vec![];
    };
    let fallback = super::context::tables(kind, &source[range.clone()], offset - range.start);
    let Ok(statements) = Parser::new(dialect(kind).as_ref())
        .with_recursion_limit(64)
        .try_with_sql(&source[range])
        .and_then(|mut p| p.parse_statements())
    else {
        return fallback;
    };
    struct Tables {
        tables: Vec<TableName>,
        ctes: Vec<String>,
    }
    impl Visitor for Tables {
        type Break = ();
        fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<()> {
            if let Some(with) = &q.with {
                self.ctes.extend(
                    with.cte_tables
                        .iter()
                        .map(|c| c.alias.name.value.to_lowercase()),
                );
            }
            ControlFlow::Continue(())
        }
        fn pre_visit_relation(&mut self, relation: &sqlparser::ast::ObjectName) -> ControlFlow<()> {
            let parts = relation
                .0
                .iter()
                .filter_map(|p| p.as_ident().map(|i| i.value.clone()))
                .collect::<Vec<_>>();
            if !parts.is_empty()
                && parts.len() <= 2
                && parts
                    .iter()
                    .all(|p| p.len() <= 256 && !p.chars().any(char::is_control))
                && !self.ctes.contains(&parts.last().unwrap().to_lowercase())
            {
                let table = TableName {
                    schema: if parts.len() == 2 {
                        parts[0].clone()
                    } else {
                        String::new()
                    },
                    name: parts.last().unwrap().clone(),
                };
                if !self.tables.contains(&table) && self.tables.len() < 8 {
                    self.tables.push(table);
                }
            }
            ControlFlow::Continue(())
        }
    }
    let mut tables = Tables {
        tables: vec![],
        ctes: vec![],
    };
    for statement in statements {
        let _ = statement.visit(&mut tables);
    }
    tables.tables
}

/// Current statement candidates and a unique caret relation, bounded and offline.
pub fn ddl_tables(
    kind: Option<DatabaseKind>,
    source: &str,
    offset: usize,
) -> (Vec<TableName>, Option<TableName>) {
    let result = super::ddl::ddl_target(kind, source, offset..offset, offset);
    (
        result.tables.into_iter().map(|table| table.table).collect(),
        result.selected,
    )
}

fn column_request_inner(
    kind: Option<DatabaseKind>,
    source: &str,
    offset: usize,
) -> Option<ColumnRequest> {
    const MARKER: &str = "__turbodbn_completion_column__";
    if kind == Some(DatabaseKind::Redis)
        || source.len() > MAX_SOURCE
        || !source.is_char_boundary(offset)
        || source.contains(MARKER)
    {
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
    let tokens = Tokenizer::new(dialect(kind).as_ref(), &source[..start])
        .tokenize()
        .ok()?;
    if !matches!(
        tokens
            .iter()
            .rev()
            .find(|t| !matches!(t, Token::Whitespace(_))),
        Some(Token::Period)
    ) {
        return None;
    }
    let mut repaired = source.to_owned();
    repaired.replace_range(start..end, MARKER);
    let statements = Parser::new(dialect(kind).as_ref())
        .with_recursion_limit(64)
        .try_with_sql(&repaired)
        .ok()?
        .parse_statements()
        .ok()?;
    struct Scope {
        bindings: Vec<(String, Option<TableName>)>,
        ctes: Vec<String>,
        opaque: bool,
    }
    struct Resolver {
        kind: Option<DatabaseKind>,
        scopes: Vec<Scope>,
        found: Option<TableName>,
    }
    impl Resolver {
        fn ident(&self, i: &Ident) -> String {
            if (self.kind == Some(DatabaseKind::PostgreSql) && i.quote_style.is_none())
                || self.kind == Some(DatabaseKind::Sqlite)
            {
                i.value.to_lowercase()
            } else {
                i.value.clone()
            }
        }
        fn binding(
            &self,
            factor: &TableFactor,
            ctes: &[String],
        ) -> Option<(String, Option<TableName>)> {
            match factor {
                TableFactor::Table {
                    name, alias, args, ..
                } => {
                    let parts = name
                        .0
                        .iter()
                        .map(|p| p.as_ident().map(|i| self.ident(i)))
                        .collect::<Option<Vec<_>>>()?;
                    if parts.is_empty()
                        || parts
                            .iter()
                            .any(|p| p.len() > 256 || p.chars().any(char::is_control))
                    {
                        return None;
                    }
                    let label = alias
                        .as_ref()
                        .map(|a| self.ident(&a.name))
                        .unwrap_or_else(|| parts.last().unwrap().clone());
                    let hidden = parts.len() == 1
                        && (ctes.contains(&parts[0])
                            || self.scopes.iter().any(|s| s.ctes.contains(&parts[0])));
                    let table = if args.is_some() || hidden || parts.len() > 2 {
                        None
                    } else {
                        Some(TableName {
                            name: parts.last()?.clone(),
                            schema: if parts.len() == 2 {
                                parts[0].clone()
                            } else {
                                String::new()
                            },
                        })
                    };
                    Some((label, table))
                }
                TableFactor::Derived {
                    alias: Some(alias), ..
                } => Some((self.ident(&alias.name), None)),
                _ => None,
            }
        }
    }
    impl Visitor for Resolver {
        type Break = ();
        fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<()> {
            let ctes = q
                .with
                .as_ref()
                .map(|w| {
                    w.cte_tables
                        .iter()
                        .map(|c| self.ident(&c.alias.name))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut bindings = Vec::new();
            let mut opaque = false;
            if let SetExpr::Select(select) = q.body.as_ref() {
                for table in &select.from {
                    for factor in std::iter::once(&table.relation)
                        .chain(table.joins.iter().map(|j| &j.relation))
                    {
                        if let Some(binding) = self.binding(factor, &ctes) {
                            bindings.push(binding);
                        } else {
                            opaque = true;
                        }
                    }
                }
            }
            self.scopes.push(Scope {
                bindings,
                ctes,
                opaque,
            });
            ControlFlow::Continue(())
        }
        fn post_visit_query(&mut self, _: &Query) -> ControlFlow<()> {
            self.scopes.pop();
            ControlFlow::Continue(())
        }
        fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
            if let Expr::CompoundIdentifier(parts) = expr
                && parts.len() == 2
                && parts[1].value == MARKER
            {
                let alias = self.ident(&parts[0]);
                for scope in self.scopes.iter().rev() {
                    let matches = scope
                        .bindings
                        .iter()
                        .filter(|(name, _)| name == &alias)
                        .collect::<Vec<_>>();
                    if !matches.is_empty() {
                        if matches.len() == 1 {
                            self.found = matches[0].1.clone();
                        }
                        break;
                    }
                    if scope.opaque {
                        break;
                    }
                }
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        }
    }
    let mut resolver = Resolver {
        kind,
        scopes: vec![],
        found: None,
    };
    for statement in statements {
        if statement.visit(&mut resolver).is_break() {
            break;
        }
    }
    Some(ColumnRequest {
        table: resolver.found?,
        prefix: source[start..offset].into(),
        range: start..end,
        local_columns: None,
    })
}

pub fn column_suggestions(
    kind: Option<DatabaseKind>,
    request: &ColumnRequest,
    columns: &[super::super::workbench::Column],
) -> Vec<Suggestion> {
    let prefix = request.prefix.to_lowercase();
    columns
        .iter()
        .filter(|c| c.name.to_lowercase().starts_with(&prefix))
        .take(1024)
        .map(|c| Suggestion {
            label: c.name.clone(),
            insert: quote(kind, &c.name),
            signature: if c.comment.is_empty() {
                c.data_type.clone()
            } else {
                format!("{} · {}", c.data_type, c.comment)
            },
            kind: SuggestionKind::Column,
            range: request.range.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ddl_uses_only_current_statement_and_caret_relation() {
        let sql =
            "SELECT * FROM unrelated; SELECT u.id,t.name FROM users u JOIN teams t ON u.id=t.id;";
        let (tables, selected) = ddl_tables(
            Some(DatabaseKind::MySql),
            sql,
            sql.find("t.name").unwrap() + 6,
        );
        assert_eq!(tables.len(), 2);
        assert_eq!(selected.unwrap().name, "teams");
        assert!(tables.iter().all(|t| t.name != "unrelated"));
        let (_, selected) = ddl_tables(
            Some(DatabaseKind::MySql),
            sql,
            sql.find("users").unwrap() + 2,
        );
        assert_eq!(selected.unwrap().name, "users");
        let (_, selected) = ddl_tables(Some(DatabaseKind::MySql), sql, sql.find("JOIN").unwrap());
        assert!(selected.is_none());
    }
    #[test]
    fn completion_uses_only_caret_statement_and_keeps_original_byte_range() {
        let source = "SELECT * FROM unrelated u; SELECT * FROM cq_user u WHERE u.na;";
        let cursor = source.find("u.na").unwrap() + 4;
        let request = column_request(Some(DatabaseKind::MySql), source, cursor).unwrap();
        assert_eq!(request.table.name, "cq_user");
        assert_eq!(request.prefix, "na");
        assert_eq!(&source[request.range], "na");
        let tables = column_sources(Some(DatabaseKind::MySql), source, cursor);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "cq_user");
    }
    #[test]
    fn explicit_column_sources_are_bounded_and_exclude_cte_names() {
        let source = "WITH filtered AS (SELECT * FROM real_table) SELECT * FROM filtered JOIN other_table o ON 1=1";
        let tables = column_sources(Some(DatabaseKind::MySql), source, source.len());
        assert!(tables.iter().all(|t| t.name != "filtered"));
        assert!(tables.iter().any(|t| t.name == "real_table"));
        assert!(tables.iter().any(|t| t.name == "other_table"));
    }
    #[test]
    fn column_metadata_is_read_without_loading_rows() {
        use crate::database::workbench::{Action, Object, Reply, Request, run};
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                use sqlx::Connection;
                let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
                    .await
                    .unwrap();
                sqlx::query(
                    "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, tenant_id INTEGER)",
                )
                .execute(&mut db)
                .await
                .unwrap();
                let result = run(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "3",
                    &Request {
                        database: "main".into(),
                        action: Action::NoteColumns(Object {
                            schema: String::new(),
                            name: "users".into(),
                            kind: "table".into(),
                            comment: String::new(),
                            parent: String::new(),
                        }),
                    },
                )
                .await
                .unwrap();
                let Reply::Schema(schema) = result else {
                    panic!("expected column metadata")
                };
                assert_eq!(
                    schema
                        .columns
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["id", "name", "tenant_id"]
                );
                assert!(schema.ddl.is_empty() && schema.indexes.values.is_empty());
            });
    }
    #[test]
    fn postgres_case_quotes_and_partial_column_ranges_are_correct() {
        let sql = "SELECT * FROM public.users AS U WHERE u.na";
        let request = column_request(Some(DatabaseKind::PostgreSql), sql, sql.len()).unwrap();
        assert_eq!(
            request.table,
            TableName {
                schema: "public".into(),
                name: "users".into()
            }
        );
        assert_eq!(request.prefix, "na");
        assert_eq!(request.range, sql.len() - 2..sql.len());
        assert!(column_request(Some(DatabaseKind::Redis), sql, sql.len()).is_none());
        assert!(column_request(None, "中文", 1).is_none());
    }
    #[test]
    fn resolves_aliases_before_and_after_caret_and_scopes() {
        for (sql, needle, name) in [
            ("SELECT * FROM `user` u WHERE u.", "u.", "user"),
            ("SELECT u. FROM users AS u", "u.", "users"),
            (
                "SELECT * FROM users u JOIN teams t ON u.id=t.id WHERE t.na",
                "t.na",
                "teams",
            ),
            (
                "SELECT * FROM outer_table u WHERE EXISTS (SELECT 1 FROM inner_table u WHERE u.)",
                "u.",
                "inner_table",
            ),
        ] {
            let offset = sql.rfind(needle).unwrap() + needle.len();
            assert_eq!(
                column_request(Some(DatabaseKind::MySql), sql, offset)
                    .unwrap()
                    .table
                    .name,
                name
            );
        }
    }
    #[test]
    fn does_not_guess_derived_cte_ambiguous_or_literal_bindings() {
        for sql in [
            "SELECT * FROM users u JOIN teams u ON true WHERE u.",
            "SELECT * FROM users u WHERE 'u.",
            "SELECT * FROM users u -- u.",
        ] {
            assert!(
                column_request(Some(DatabaseKind::MySql), sql, sql.len()).is_none(),
                "{sql}"
            );
        }
    }
    #[test]
    fn columns_keep_qualifier_and_do_not_truncate_at_keyword_limit() {
        let sql = "SELECT * FROM users u WHERE u.";
        let request = column_request(Some(DatabaseKind::MySql), sql, sql.len()).unwrap();
        let columns = (0..100)
            .map(|i| super::super::super::workbench::Column {
                name: format!("列`{i}"),
                data_type: "INT".into(),
                comment: String::new(),
                primary: false,
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let items = column_suggestions(Some(DatabaseKind::MySql), &request, &columns);
        assert_eq!(items.len(), 100);
        assert_eq!(items[0].insert, "`列``0`");
        assert_eq!(items[0].range, sql.len()..sql.len());
    }
}
