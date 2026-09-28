use super::*;
use sqlx::Connection;
#[tokio::test]
async fn sqlite_pages_schema_and_guard() {
    let mut c = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE demo(id INTEGER PRIMARY KEY, text TEXT, parent_id INTEGER REFERENCES parent(id)); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<205) INSERT INTO demo(id,text) SELECT x,CASE WHEN x=1 THEN NULL ELSE '' END FROM n;").execute(&mut c).await.unwrap();
    let o = Object {
        schema: String::new(),
        name: "demo".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let request = |action| Request {
        database: "test".into(),
        action,
    };
    let Reply::Page { rows, more, .. } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &request(Action::Page {
            object: o.clone(),
            offset: 0,
            page_size: 100,
        }),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(rows.values.len(), 100);
    assert!(more);
    assert_eq!(rows.values[0][1], None);
    assert_eq!(rows.values[1][1], Some(String::new()));
    let Reply::Schema(s) = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &request(Action::Describe(o.clone())),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(s.foreign_keys.values.len(), 1);
    sqlx::raw_sql("ALTER TABLE demo ADD COLUMN changed TEXT")
        .execute(&mut c)
        .await
        .unwrap();
    assert!(
        run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &request(Action::Execute {
                sql: "DROP TABLE demo".into(),
                guard: Some((o, s.ddl))
            })
        )
        .await
        .is_err()
    );
}

#[test]
fn mysql_column_edit_preserves_constraints_and_default() {
    let object = Object {
        schema: String::new(),
        name: "order`details".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let old = Column {
        name: "label".into(),
        data_type: "varchar(80)".into(),
        comment: "old".into(),
        primary: false,
    };
    let ddl = "CREATE TABLE `order``details` (\n  `label` varchar(80) NOT NULL DEFAULT 'COMMENT unchanged' COMMENT 'old',\n  PRIMARY KEY (`label`)\n) ENGINE=InnoDB";
    let sql = change_column(
        DatabaseKind::MySql,
        "test",
        &object,
        &old,
        ddl,
        "title",
        "varchar(120)",
        "new ' comment",
    )
    .unwrap();
    assert!(sql.contains(
        "CHANGE COLUMN `label` `title` varchar(120) NOT NULL DEFAULT 'COMMENT unchanged'"
    ));
    assert!(sql.contains("COMMENT 'new '' comment'"));
    assert!(!sql.contains("COMMENT 'old'"));
    assert!(
        change_column(
            DatabaseKind::MySql,
            "test",
            &object,
            &old,
            ddl,
            "title",
            "int); DROP TABLE x;--",
            "old"
        )
        .is_err()
    );
}
#[tokio::test]
async fn csv_round_trip_handles_null_empty_quotes_newlines_and_slashes() {
    use sqlx::Connection;
    let mut c = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE demo(value TEXT)")
        .execute(&mut c)
        .await
        .unwrap();
    let rows = Rows {
        columns: vec!["value".into()],
        values: vec![
            vec![None],
            vec![Some(String::new())],
            vec![Some("\\N".into())],
            vec![Some("a,\"quoted\"\nline".into())],
            vec![Some("'); DROP TABLE demo; --".into())],
        ],
    };
    let csv = export_csv(&rows).unwrap();
    let object = Object {
        schema: String::new(),
        name: "demo".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let columns = vec![Column {
        name: "value".into(),
        data_type: "TEXT".into(),
        comment: String::new(),
        primary: false,
    }];
    let sql = import_csv(DatabaseKind::Sqlite, "", &object, &columns, &csv).unwrap();
    c.query(&sql).await.unwrap();
    let actual = c
        .query("SELECT value FROM demo ORDER BY rowid")
        .await
        .unwrap();
    assert_eq!(actual.values, rows.values);
}
#[test]
fn capability_lists_do_not_invent_legacy_objects() {
    assert_eq!(
        categories(DatabaseKind::MySql, "4.1.22"),
        vec!["table", "index"]
    );
    assert!(!categories(DatabaseKind::Sqlite, "").contains(&"procedure"));
    assert!(categories(DatabaseKind::Redis, "").is_empty());
}

#[tokio::test]
async fn grid_keyset_filter_count_and_atomic_edits() {
    use std::collections::BTreeMap;
    let mut c = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE grid(a INTEGER,b INTEGER,label TEXT DEFAULT 'default',PRIMARY KEY(a,b)); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<205) INSERT INTO grid SELECT x/10,x%10,'row'||x FROM n;").execute(&mut c).await.unwrap();
    let o = Object {
        schema: String::new(),
        name: "grid".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let req = |action| Request {
        database: "fixture".into(),
        action,
    };
    let Reply::GridPage {
        rows,
        schema,
        next,
        keyset,
        more,
        ..
    } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &req(Action::GridPage {
            object: o.clone(),
            options: PageOptions::default(),
        }),
    )
    .await
    .unwrap()
    else {
        panic!()
    };
    assert!(keyset && more);
    assert_eq!(rows.values.len(), 200);
    sqlx::raw_sql("DELETE FROM grid WHERE a=0 AND b=1")
        .execute(&mut c)
        .await
        .unwrap();
    let Reply::GridPage {
        rows: second, sql, ..
    } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &req(Action::GridPage {
            object: o.clone(),
            options: PageOptions {
                offset: 200,
                after: next,
                ..Default::default()
            },
        }),
    )
    .await
    .unwrap()
    else {
        panic!()
    };
    assert_eq!(second.values[0][2].as_deref(), Some("row201"));
    assert!(!sql.contains("OFFSET"));
    let Reply::Count(count) = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &req(Action::Count {
            object: o.clone(),
            filter: None,
        }),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(count, 204);
    let injection = Filter {
        column: "label".into(),
        operator: "eq".into(),
        value: "' OR 1=1 --".into(),
    };
    let Reply::GridPage {
        rows: filtered,
        keyset,
        ..
    } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &req(Action::GridPage {
            object: o.clone(),
            options: PageOptions {
                filter: Some(injection.into()),
                sort: Some(("label".into(), true)),
                ..Default::default()
            },
        }),
    )
    .await
    .unwrap()
    else {
        panic!()
    };
    assert!(filtered.values.is_empty());
    assert!(!keyset);
    let original = second.values[0].clone();
    let update = Change {
        original: Some(original.clone()),
        values: Some(BTreeMap::from([("label".into(), Some("edited".into()))])),
    };
    let duplicate = Change {
        original: None,
        values: Some(BTreeMap::from([
            ("a".into(), original[0].clone()),
            ("b".into(), original[1].clone()),
        ])),
    };
    assert!(
        run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &req(Action::Commit {
                object: o.clone(),
                ddl: schema.ddl.clone(),
                changes: vec![update.clone(), duplicate]
            })
        )
        .await
        .is_err()
    );
    let label: String = sqlx::query_scalar("SELECT label FROM grid WHERE a=20 AND b=1")
        .fetch_one(&mut c)
        .await
        .unwrap();
    assert_eq!(label, "row201");
    run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &req(Action::Commit {
            object: o.clone(),
            ddl: schema.ddl.clone(),
            changes: vec![update.clone()],
        }),
    )
    .await
    .unwrap();
    assert!(
        run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &req(Action::Commit {
                object: o.clone(),
                ddl: schema.ddl,
                changes: vec![update]
            })
        )
        .await
        .is_err()
    );
    sqlx::raw_sql("CREATE TABLE no_key(value TEXT); INSERT INTO no_key VALUES ('x')")
        .execute(&mut c)
        .await
        .unwrap();
    let mut no_key = o;
    no_key.name = "no_key".into();
    let Reply::GridPage { keyset, sql, .. } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &req(Action::GridPage {
            object: no_key,
            options: PageOptions::default(),
        }),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert!(!keyset && sql.contains("OFFSET"));
}
#[test]
fn row_copy_formats_escape_strings_and_preserve_null() {
    let object = Object {
        schema: String::new(),
        name: "t".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let rows = Rows {
        columns: vec!["a".into(), "b".into()],
        values: vec![vec![Some("one,\n'|two".into()), None]],
    };
    let json = copy_row(DatabaseKind::Sqlite, "main", &object, &rows, 0, "json").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed["b"].is_null());
    assert!(
        copy_row(DatabaseKind::Sqlite, "main", &object, &rows, 0, "sql")
            .unwrap()
            .contains("''|two")
    );
    assert!(
        copy_row(DatabaseKind::Sqlite, "main", &object, &rows, 0, "md")
            .unwrap()
            .contains("<br>")
    );
    assert!(
        copy_row(DatabaseKind::Sqlite, "main", &object, &rows, 0, "csv")
            .unwrap()
            .contains("\"one,")
    );
}

#[test]
fn mysql_literals_do_not_replace_four_byte_unicode() {
    assert!(literal_for(DatabaseKind::MySql, "😀").ends_with("USING utf8mb4)"));
    assert!(literal_for(DatabaseKind::MySql, "数据").ends_with("USING utf8)"));
}

#[tokio::test]
async fn grid_defaults_insert_and_delete_are_transactional() {
    use std::collections::BTreeMap;
    let mut c = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE keyed(id INTEGER PRIMARY KEY,name TEXT DEFAULT 'kept'); CREATE TABLE plain(name TEXT DEFAULT 'plain');").execute(&mut c).await.unwrap();
    for name in ["keyed", "plain"] {
        let o = Object {
            schema: String::new(),
            name: name.into(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        let req = |action| Request {
            database: "fixture".into(),
            action,
        };
        let Reply::GridPage { schema, .. } = run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &req(Action::GridPage {
                object: o.clone(),
                options: PageOptions::default(),
            }),
        )
        .await
        .unwrap() else {
            panic!()
        };
        run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &req(Action::Commit {
                object: o.clone(),
                ddl: schema.ddl.clone(),
                changes: vec![Change {
                    original: None,
                    values: Some(BTreeMap::new()),
                }],
            }),
        )
        .await
        .unwrap();
        let Reply::GridPage { rows, .. } = run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &req(Action::GridPage {
                object: o.clone(),
                options: PageOptions::default(),
            }),
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(rows.values.len(), 1);
        if name == "keyed" {
            assert_eq!(rows.values[0][1].as_deref(), Some("kept"));
            run(
                &mut c,
                DatabaseKind::Sqlite,
                "",
                &req(Action::Commit {
                    object: o.clone(),
                    ddl: schema.ddl,
                    changes: vec![Change {
                        original: Some(rows.values[0].clone()),
                        values: None,
                    }],
                }),
            )
            .await
            .unwrap();
            let Reply::Count(n) = run(
                &mut c,
                DatabaseKind::Sqlite,
                "",
                &req(Action::Count {
                    object: o,
                    filter: None,
                }),
            )
            .await
            .unwrap() else {
                panic!()
            };
            assert_eq!(n, 0);
        } else {
            assert_eq!(rows.values[0][0].as_deref(), Some("plain"));
        }
    }
}

#[tokio::test]
async fn grid_page_sizes_and_offset_jumps_are_bounded() {
    let mut c = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE sized(id INTEGER PRIMARY KEY); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<401) INSERT INTO sized SELECT x FROM n;").execute(&mut c).await.unwrap();
    let o = Object {
        schema: String::new(),
        name: "sized".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    for (size, offset, len, more) in [
        (1, 0, 1, true),
        (50, 0, 50, true),
        (200, 200, 200, true),
        (200, 400, 1, false),
        (1000, 0, 401, false),
    ] {
        let result = run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &Request {
                database: "fixture".into(),
                action: Action::GridPage {
                    object: o.clone(),
                    options: PageOptions {
                        page_size: size,
                        offset,
                        ..Default::default()
                    },
                },
            },
        )
        .await
        .unwrap();
        let Reply::GridPage {
            rows,
            more: actual,
            sql,
            keyset,
            ..
        } = result
        else {
            panic!()
        };
        assert_eq!(rows.values.len(), len);
        assert_eq!(actual, more);
        assert_eq!(
            rows.values[0][0].as_deref(),
            Some((offset + 1).to_string().as_str())
        );
        assert!(sql.contains(&format!("LIMIT {}", size + 1)));
        assert_eq!(keyset, offset == 0);
    }
    for size in [0, 1001, usize::MAX] {
        assert!(
            run(
                &mut c,
                DatabaseKind::Sqlite,
                "",
                &Request {
                    database: "fixture".into(),
                    action: Action::GridPage {
                        object: o.clone(),
                        options: PageOptions {
                            page_size: size,
                            ..Default::default()
                        }
                    }
                }
            )
            .await
            .is_err()
        );
    }
    for (operator, value, expected) in [("ge", "400", 2), ("le", "2", 2), ("null", "", 0)] {
        let Reply::GridPage { rows, .. } = run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &Request {
                database: "fixture".into(),
                action: Action::GridPage {
                    object: o.clone(),
                    options: PageOptions {
                        filter: Some(
                            Filter {
                                column: "id".into(),
                                operator: operator.into(),
                                value: value.into(),
                            }
                            .into(),
                        ),
                        ..Default::default()
                    },
                },
            },
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(rows.values.len(), expected);
    }
    let old: PageOptions =
        serde_json::from_str(r#"{"offset":0,"after":null,"sort":null,"filter":null}"#).unwrap();
    assert_eq!(old.page_size, 200);
}

#[tokio::test]
async fn combined_filters_share_count_and_cursor_semantics() {
    let mut c = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE combined(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO combined VALUES (1,'a'),(2,NULL),(3,'b'),(4,'c'),(5,'d'),(6,'e')").execute(&mut c).await.unwrap();
    let object = Object {
        schema: String::new(),
        name: "combined".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let filters = vec![
        Filter {
            column: "id".into(),
            operator: "le".into(),
            value: "2".into(),
        },
        Filter {
            column: "id".into(),
            operator: "ge".into(),
            value: "5".into(),
        },
    ];
    let request = |action| Request {
        database: "fixture".into(),
        action,
    };
    let group = FilterSet::Group {
        join: FilterJoin::Or,
        conditions: filters.clone(),
    };
    let Reply::GridPage { rows, next, .. } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &request(Action::GridPage {
            object: object.clone(),
            options: PageOptions {
                page_size: 2,
                filter: Some(group.clone()),
                ..Default::default()
            },
        }),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        rows.values
            .iter()
            .map(|r| r[0].as_deref())
            .collect::<Vec<_>>(),
        vec![Some("1"), Some("2")]
    );
    let Reply::GridPage { rows, more, .. } = run(
        &mut c,
        DatabaseKind::Sqlite,
        "",
        &request(Action::GridPage {
            object: object.clone(),
            options: PageOptions {
                page_size: 2,
                offset: 2,
                after: next,
                filter: Some(group.clone()),
                ..Default::default()
            },
        }),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        rows.values
            .iter()
            .map(|r| r[0].as_deref())
            .collect::<Vec<_>>(),
        vec![Some("5"), Some("6")]
    );
    assert!(!more);
    for (join, expected) in [(FilterJoin::Or, 4), (FilterJoin::And, 0)] {
        let Reply::Count(count) = run(
            &mut c,
            DatabaseKind::Sqlite,
            "",
            &request(Action::Count {
                object: object.clone(),
                filter: Some(FilterSet::Group {
                    join,
                    conditions: filters.clone(),
                }),
            }),
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(count, expected);
    }
    for conditions in [
        Vec::new(),
        vec![filters[0].clone(); 21],
        vec![Filter {
            column: "id) OR 1=1 --".into(),
            operator: "eq".into(),
            value: "1".into(),
        }],
    ] {
        assert!(
            run(
                &mut c,
                DatabaseKind::Sqlite,
                "",
                &request(Action::Count {
                    object: object.clone(),
                    filter: Some(FilterSet::Group {
                        join: FilterJoin::Or,
                        conditions
                    })
                })
            )
            .await
            .is_err()
        );
    }
    let legacy: FilterSet =
        serde_json::from_str(r#"{"column":"id","operator":"eq","value":"1"}"#).unwrap();
    assert_eq!(legacy.conditions().len(), 1);
}
#[test]
fn copied_cell_where_quotes_identifiers_values_and_null() {
    let column = Column {
        name: "odd\"name".into(),
        data_type: "TEXT".into(),
        comment: String::new(),
        primary: false,
    };
    assert_eq!(
        cell_where(DatabaseKind::Sqlite, &column, Some("O'Reilly")).unwrap(),
        "WHERE \"odd\"\"name\"='O''Reilly'"
    );
    assert_eq!(
        cell_where(DatabaseKind::PostgreSql, &column, None).unwrap(),
        "WHERE \"odd\"\"name\" IS NULL"
    );
    let text = Column {
        name: "sync_mode".into(),
        data_type: "varchar(32)".into(),
        ..column.clone()
    };
    assert_eq!(
        cell_where(DatabaseKind::MySql, &text, Some("full")).unwrap(),
        "WHERE `sync_mode`='full'"
    );
    assert_eq!(
        cell_where(DatabaseKind::MySql, &text, Some("O'Reilly")).unwrap(),
        "WHERE `sync_mode`='O''Reilly'"
    );
    assert!(
        cell_where(DatabaseKind::MySql, &text, Some(r"a\b"))
            .unwrap()
            .contains("CONVERT(")
    );
    let number = Column {
        name: "sync_source".into(),
        data_type: "bigint unsigned".into(),
        ..column.clone()
    };
    assert_eq!(
        cell_where(DatabaseKind::MySql, &number, Some("2")).unwrap(),
        "WHERE `sync_source`= 2"
    );
    assert_eq!(
        cell_where(DatabaseKind::MySql, &number, Some("18446744073709551615")).unwrap(),
        "WHERE `sync_source`= 18446744073709551615"
    );
    assert!(cell_where(DatabaseKind::MySql, &number, Some("2 OR 1=1")).is_err());
    assert!(cell_where(DatabaseKind::MySql, &number, Some("NaN")).is_err());
    let interval = Column {
        data_type: "interval".into(),
        ..column.clone()
    };
    assert!(
        cell_where(DatabaseKind::PostgreSql, &interval, Some("1 day"))
            .unwrap()
            .ends_with("=$turbodbnote$1 day$turbodbnote$")
    );
    let binary = Column {
        data_type: "BLOB".into(),
        ..column
    };
    assert!(cell_where(DatabaseKind::Sqlite, &binary, Some("preview")).is_err());
}
