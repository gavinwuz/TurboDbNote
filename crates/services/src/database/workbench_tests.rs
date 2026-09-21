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
