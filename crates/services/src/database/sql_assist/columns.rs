use super::*;
use sqlparser::ast::{Expr, Ident, Query, SetExpr, TableFactor, Visit, Visitor};
use std::ops::ControlFlow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnRequest {
    pub table: TableName,
    pub prefix: String,
    pub range: Range<usize>,
}

/// Parse a temporary identifier at the caret, never execute the repaired text.
/// Derived tables/CTEs and ambiguous bindings are deliberately not guessed.
pub fn column_request(
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
            "SELECT * FROM (SELECT id FROM users) u WHERE u.",
            "WITH u AS (SELECT * FROM users) SELECT * FROM u WHERE u.",
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
            })
            .collect::<Vec<_>>();
        let items = column_suggestions(Some(DatabaseKind::MySql), &request, &columns);
        assert_eq!(items.len(), 100);
        assert_eq!(items[0].insert, "`列``0`");
        assert_eq!(items[0].range, sql.len()..sql.len());
    }
}
