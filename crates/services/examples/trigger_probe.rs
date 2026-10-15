//! Explicit loopback-only isolated MySQL fixture; never run against an existing user server.
use sqlx::{
    Connection,
    mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode},
};
use turbodbn_services::{
    DatabaseKind,
    database::notebook_query::{execute_script, prepare_script, script_diagnostics},
};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(
        args.get(1).map(String::as_str),
        Some("--isolated"),
        "Explicit isolated fixture required"
    );
    let port: u16 = args[0].parse().expect("Isolated loopback port");
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
        let options=MySqlConnectOptions::new().host("127.0.0.1").port(port).username("root").ssl_mode(MySqlSslMode::Disabled);
        let mut c=MySqlConnection::connect_with(&options).await.expect("Owned server connection failed");
        for sql in ["CREATE DATABASE IF NOT EXISTS trigger_fixture","USE trigger_fixture","CREATE TABLE IF NOT EXISTS items(id INT)","DROP TRIGGER IF EXISTS sample_trigger","CREATE TRIGGER sample_trigger BEFORE INSERT ON items FOR EACH ROW SET NEW.id=NEW.id","CREATE USER IF NOT EXISTS 'trigger_probe'@'127.0.0.1'","REVOKE ALL PRIVILEGES, GRANT OPTION FROM 'trigger_probe'@'127.0.0.1'","GRANT SELECT ON trigger_fixture.* TO 'trigger_probe'@'127.0.0.1'"] { sqlx::raw_sql(sql).execute(&mut c).await.expect("Fixture setup failed"); }
        let version:String=sqlx::query_scalar("SELECT VERSION()").fetch_one(&mut c).await.unwrap();
        let sql="-- fixture comment\nDROP TRIGGER IF EXISTS `sample_trigger`;";
        for _ in 0..2 {
            let plan=prepare_script(&mut c,DatabaseKind::MySql,"trigger_fixture",&version,sql).await.expect("Prepare failed");
            let result=execute_script(&mut c,DatabaseKind::MySql,"trigger_fixture",&version,&plan,true).await.expect("Drop failed");
            assert_eq!(result.execution.unwrap().operation,"DROP TRIGGER");
        }
        sqlx::raw_sql("CREATE TRIGGER sample_trigger BEFORE INSERT ON items FOR EACH ROW SET NEW.id=NEW.id").execute(&mut c).await.unwrap();
        let mut limited=MySqlConnection::connect_with(&options.clone().username("trigger_probe").database("trigger_fixture")).await.unwrap();
        let plan=prepare_script(&mut limited,DatabaseKind::MySql,"trigger_fixture",&version,sql).await.unwrap();
        let error=execute_script(&mut limited,DatabaseKind::MySql,"trigger_fixture",&version,&plan,true).await.unwrap_err();
        assert!(error=="mysql.error.command_denied"||error=="mysql.error.privilege_1227","Permission must retain a safe category");
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA='trigger_fixture'").fetch_one(&mut c).await.unwrap();assert_eq!(count,1);
        let report=script_diagnostics(&mut limited,DatabaseKind::MySql).await.unwrap();
        let enabled:i64=sqlx::query_scalar("SELECT @@GLOBAL.log_bin").fetch_one(&mut c).await.unwrap();
        assert_eq!(report.binary_log,enabled.to_string());
        sqlx::raw_sql("GRANT TRIGGER ON trigger_fixture.* TO 'trigger_probe'@'127.0.0.1'").execute(&mut c).await.unwrap();
        limited.close().await.unwrap();
        let mut limited=MySqlConnection::connect_with(&options.clone().username("trigger_probe").database("trigger_fixture")).await.unwrap();
        let plan=prepare_script(&mut limited,DatabaseKind::MySql,"trigger_fixture",&version,sql).await.unwrap();
        execute_script(&mut limited,DatabaseKind::MySql,"trigger_fixture",&version,&plan,true).await.expect("TRIGGER grant should permit deletion");
        if enabled==1 {
            let failed=sqlx::raw_sql("CREATE TRIGGER sample_trigger BEFORE INSERT ON items FOR EACH ROW SET NEW.id=NEW.id").execute(&mut limited).await;
            let code=failed.err().and_then(|e|match e {sqlx::Error::Database(e)=>e.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>().map(|e|e.number()),_=>None});
            assert_eq!(code,Some(1419));
            println!("Binlog ON: TRIGGER-only account can DROP, CREATE returns 1419; read-only diagnostics verified");
        }
        if let Some(folder)=args.get(2) {
            use turbodbn_core::notebook::{Note,Cell,CellContent,SqlDialect};
            use std::sync::atomic::AtomicBool;
            let root=std::path::Path::new(folder);std::fs::create_dir_all(root).unwrap();
            let path=root.join("comments.note");assert!(!path.exists(),"Do not overwrite a GUI fixture");
            let source=root.join("comments.sql");
            std::fs::write(&source,"-- 结构准备说明：注释不应遮住 SQL\n# 第二行说明\n/* 普通块注释 */\nDROP TRIGGER IF EXISTS `sample_trigger`;\n").unwrap();
            sqlx::raw_sql("CREATE TRIGGER sample_trigger BEFORE INSERT ON items FOR EACH ROW SET NEW.id=NEW.id").execute(&mut c).await.unwrap();
            sqlx::raw_sql("REVOKE TRIGGER ON trigger_fixture.* FROM 'trigger_probe'@'127.0.0.1'").execute(&mut c).await.unwrap();
            let cancel=AtomicBool::new(false);
            let prepared=turbodbn_services::sql_script::prepare(&source,SqlDialect::MySql,&cancel,|_,_|{}).unwrap();
            prepared.publish(&path,&cancel).unwrap();
            let mut note=Note::new("Script source verification");note.default_database=Some("trigger_fixture".into());
            note.cells.push(Cell::new("comments.sql",CellContent::SqlScript{script:prepared.script.clone(),connection_id:None,database:Some("trigger_fixture".into())}));
            std::fs::write(path,note.to_json().unwrap()).unwrap();
            println!("Created bounded GUI source fixture; no credentials are stored in the Note");
        }
        println!("Owned MySQL: commented DROP TRIGGER, absent IF EXISTS, permission rejection and object preservation passed");
    });
}
