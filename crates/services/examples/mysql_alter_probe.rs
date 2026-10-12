//! Run only against an explicitly owned loopback fixture started with --no-defaults.
use sqlx::{
    Connection, Row,
    mysql::{MySqlConnectOptions, MySqlSslMode},
};
use turbodbn_services::{
    DatabaseKind,
    database::notebook_query::{execute, prepare},
};

async fn checks(
    c: &mut sqlx::MySqlConnection,
    db: &str,
    version: &str,
) -> Result<(), &'static str> {
    let grants = sqlx::raw_sql("SHOW GRANTS")
        .fetch_all(&mut *c)
        .await
        .map_err(|_| "grants check failed")?;
    if !grants.iter().any(|r| {
        r.try_get::<String, _>(0).is_ok_and(|s| {
            s.split_once(" ON ").is_some_and(|(grants, scope)| {
                scope.starts_with("*.* TO ")
                    && grants.strip_prefix("GRANT ").is_some_and(|list| {
                        list.split(',')
                            .any(|grant| matches!(grant.trim(), "ALL PRIVILEGES" | "ALTER"))
                    })
            })
        })
    }) {
        return Err("fixture requires administrative grants");
    }
    sqlx::raw_sql("CREATE TABLE items(id INT PRIMARY KEY)")
        .execute(&mut *c)
        .await
        .map_err(|_| "setup failed")?;
    let old_query = format!("SHOW TABLES FROM `{db}` LIKE CONVERT(X'6974656d73' USING utf8)");
    let error = sqlx::raw_sql(&old_query)
        .fetch_all(&mut *c)
        .await
        .err()
        .ok_or("old preflight unexpectedly succeeded")?;
    let code = error
        .as_database_error()
        .and_then(|e| e.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>())
        .map(|e| e.number());
    if code != Some(1064) {
        return Err("old preflight did not produce 1064");
    }
    println!("PASS: administrative account reproduces old preflight error 1064");
    for (index, mode) in ["", "NO_BACKSLASH_ESCAPES"].iter().enumerate() {
        sqlx::raw_sql(&format!("SET SESSION sql_mode='{mode}'"))
            .execute(&mut *c)
            .await
            .map_err(|_| "mode setup failed")?;
        let sql = format!("ALTER TABLE items ADD COLUMN label{index} VARCHAR(40) DEFAULT 'a;b'");
        let plan = prepare(c, DatabaseKind::MySql, db, version, &sql)
            .await
            .map_err(|_| "ALTER preparation failed")?;
        if plan.sql != sql || !plan.confirm || plan.read_only {
            return Err("ALTER plan changed");
        }
        if execute(c, DatabaseKind::MySql, db, version, &plan, false, (0, 20))
            .await
            .is_ok()
        {
            return Err("unconfirmed ALTER executed");
        }
        execute(c, DatabaseKind::MySql, db, version, &plan, true, (0, 20))
            .await
            .map_err(|_| "confirmed ALTER failed")?;
        let exists:i64=sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME='items' AND COLUMN_NAME=?")
            .bind(db).bind(format!("label{index}")).fetch_one(&mut *c).await.map_err(|_|"column verification failed")?;
        if exists != 1 {
            return Err("ALTER did not add its column");
        }
    }
    println!("PASS: confirmed ALTER under ordinary and NO_BACKSLASH_ESCAPES modes");
    let plan = prepare(
        c,
        DatabaseKind::MySql,
        db,
        version,
        "ALTER TABLE items ADD COLUMN blocked INT",
    )
    .await
    .map_err(|_| "stale preparation failed")?;
    sqlx::raw_sql("ALTER TABLE items ADD COLUMN concurrent INT")
        .execute(&mut *c)
        .await
        .map_err(|_| "concurrent setup failed")?;
    if execute(c, DatabaseKind::MySql, db, version, &plan, true, (0, 20))
        .await
        .is_ok()
    {
        return Err("stale plan executed");
    }
    let create = "CREATE TABLE fresh(id INT)";
    let plan = prepare(c, DatabaseKind::MySql, db, version, create)
        .await
        .map_err(|_| "missing-table preparation failed")?;
    if !plan.guards.iter().all(|g| g.stamp.is_none()) {
        return Err("missing table was not recognized");
    }
    execute(c, DatabaseKind::MySql, db, version, &plan, true, (0, 20))
        .await
        .map_err(|_| "CREATE failed")?;
    sqlx::raw_sql("CREATE TABLE `odd_%'x``y`(id INT)")
        .execute(&mut *c)
        .await
        .map_err(|_| "quoted-name setup failed")?;
    let sql = "ALTER TABLE `odd_%'x``y` ADD COLUMN extra INT";
    let plan = prepare(c, DatabaseKind::MySql, db, version, sql)
        .await
        .map_err(|_| "quoted-name preparation failed")?;
    execute(c, DatabaseKind::MySql, db, version, &plan, true, (0, 20))
        .await
        .map_err(|_| "quoted-name ALTER failed")?;
    let invalid = "ALTER TABLE items ADD COLUMN invalid_type UNKNOWN_TYPE";
    let plan = prepare(c, DatabaseKind::MySql, db, version, invalid)
        .await
        .map_err(|_| "invalid-SQL preparation failed")?;
    if execute(c, DatabaseKind::MySql, db, version, &plan, true, (0, 20))
        .await
        .err()
        .as_deref()
        != Some("mysql.error.syntax_1064")
    {
        return Err("syntax category was not preserved");
    }
    println!(
        "PASS: stale guard, missing table, exact quoted identifiers and syntax classification"
    );
    Ok(())
}

async fn run(port: u16, expected_data: &str) -> Result<(), &'static str> {
    let options = MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(port)
        .username("root")
        .ssl_mode(MySqlSslMode::Disabled);
    let mut c = sqlx::MySqlConnection::connect_with(&options)
        .await
        .map_err(|_| "fixture connection failed")?;
    let actual: String = sqlx::query_scalar("SELECT @@datadir")
        .fetch_one(&mut c)
        .await
        .map_err(|_| "fixture identity check failed")?;
    let actual = dunce::canonicalize(actual).map_err(|_| "fixture path unavailable")?;
    let expected =
        dunce::canonicalize(expected_data).map_err(|_| "expected fixture path unavailable")?;
    if actual != expected {
        return Err("refusing a different data directory");
    }
    let version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&mut c)
        .await
        .map_err(|_| "fixture version check failed")?;
    let display: String = version
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    println!("Fixture server version: {display}");
    let db = format!("turbodbnote_alter_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE DATABASE `{db}`"))
        .execute(&mut c)
        .await
        .map_err(|_| "fixture database creation failed")?;
    let result = async {
        sqlx::raw_sql(&format!("USE `{db}`"))
            .execute(&mut c)
            .await
            .map_err(|_| "fixture selection failed")?;
        checks(&mut c, &db, &version).await
    }
    .await;
    let cleanup = sqlx::raw_sql(&format!("DROP DATABASE `{db}`"))
        .execute(&mut c)
        .await
        .map_err(|_| "owned fixture cleanup failed");
    result.and(cleanup.map(|_| ()))
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 || args[0] != "--isolated-fixture" {
        eprintln!("Expected --isolated-fixture PORT OWNED_DATA_DIRECTORY");
        std::process::exit(2);
    }
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(args[1].parse().expect("fixture port"), &args[2]));
    if let Err(category) = result {
        eprintln!("FAIL: {category}");
        std::process::exit(1);
    }
}
