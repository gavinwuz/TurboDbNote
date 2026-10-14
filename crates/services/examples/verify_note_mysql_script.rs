//! Run only against a newly initialized, loopback-only, isolated test server.
use sqlx::{
    ConnectOptions, Connection,
    mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode},
};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Duration};
use turbodbn_core::{Cell, CellContent, Note, notebook::SqlDialect};
use turbodbn_services::{
    DatabaseKind,
    database::{
        Operation, OperationKind, OperationResult, notebook_query as note,
        workbench::{Action, Reply, Request},
    },
    mysql::{Driver, Event, Profile, Session, TlsMode},
    sql_script,
};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let port = args
        .first()
        .and_then(|v| v.parse::<u16>().ok())
        .expect("An isolated server port is required");
    let directory = args
        .get(1)
        .map(PathBuf::from)
        .expect("An isolated fixture directory is required");
    let directory = if directory.is_absolute() {
        directory
    } else {
        std::env::current_dir().unwrap().join(directory)
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
        let opts=MySqlConnectOptions::new().host("127.0.0.1").port(port).username("root").ssl_mode(MySqlSslMode::Disabled).charset("utf8mb4").disable_statement_logging();
        let mut db=MySqlConnection::connect_with(&opts).await.expect("Cannot connect to isolated server");
        sqlx::query("CREATE DATABASE turbodbnote_script_fixture CHARACTER SET utf8mb4").execute(&mut db).await.expect("Fixture already exists or creation failed; refusing to reuse it");
        sqlx::raw_sql("USE turbodbnote_script_fixture").execute(&mut db).await.unwrap();
        let version:String=sqlx::query_scalar("SELECT VERSION()").fetch_one(&mut db).await.unwrap();
        let script="USE turbodbnote_script_fixture;\n/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;\n/*!40101 SET NAMES utf8mb4 */;\nSET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0;\nCREATE TABLE imported(id INT PRIMARY KEY,v TEXT);\nLOCK TABLES imported WRITE;\n/*!40000 ALTER TABLE imported DISABLE KEYS */;\nINSERT INTO imported VALUES(1,'汉字\\\'值');\n/*!40000 ALTER TABLE imported ENABLE KEYS */;\nUNLOCK TABLES;\nSET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS;\n/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;\nSET autocommit=0;\nINSERT INTO imported VALUES(2,'pending');\nCOMMIT;\nSET autocommit=1;\nSELECT * FROM imported;";
        std::fs::create_dir_all(&directory).unwrap();let source=directory.join("mysql-dump.sql");std::fs::write(&source,script).unwrap();
        let imported=sql_script::prepare(&source,SqlDialect::MySql,&AtomicBool::new(false),|_,_|{}).unwrap();assert_eq!(imported.script.unsupported,0);
        let path=directory.join("mysql-dump.note");imported.publish(&path,&AtomicBool::new(false)).unwrap();
        let mut reader=sql_script::freeze(&path,&imported.script,&AtomicBool::new(false)).unwrap();
        let mut sequence=turbodbn_services::database::workbench::script_context::Sequence::default();
        for n in 0..imported.script.statements {let sql=reader.statement(n).unwrap();sequence.push(DatabaseKind::MySql,&sql,!note::validate_script_statement(DatabaseKind::MySql,&sql).unwrap()).unwrap();}
        sequence.finish().unwrap();
        for n in 0..imported.script.statements {
            let sql=reader.statement(n).unwrap();let plan=note::prepare_script(&mut db,DatabaseKind::MySql,"turbodbnote_script_fixture",&version,&sql).await.unwrap();
            if plan.confirm {assert!(note::execute_script(&mut db,DatabaseKind::MySql,"turbodbnote_script_fixture",&version,&plan,false).await.is_err());}
            note::execute_script(&mut db,DatabaseKind::MySql,"turbodbnote_script_fixture",&version,&plan,true).await.unwrap();
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM imported").fetch_one(&mut db).await.unwrap();assert_eq!(count,2);
        let value:String=sqlx::query_scalar("SELECT v FROM imported WHERE id=1").fetch_one(&mut db).await.unwrap();assert_eq!(value,"汉字'值");
        let bad=note::prepare_script(&mut db,DatabaseKind::MySql,"turbodbnote_script_fixture",&version,"USE other").await;assert!(bad.is_err());
        let bulk=format!("INSERT INTO imported VALUES {}",(3..1003).map(|i|format!("({i},'{}')","x".repeat(80))).collect::<Vec<_>>().join(","));assert!(bulk.len()>60_000);
        let plan=note::prepare_script(&mut db,DatabaseKind::MySql,"turbodbnote_script_fixture",&version,&bulk).await.unwrap();note::execute_script(&mut db,DatabaseKind::MySql,"turbodbnote_script_fixture",&version,&plan,true).await.unwrap();
        // Verify the actual adapter bypasses per-operation charset resets and releases locks on drop.
        let profile=Profile{name:"Isolated script verification".into(),driver:Driver::Modern,target_version:version.clone(),host:"127.0.0.1".into(),port,user:"root".into(),database:"turbodbnote_script_fixture".into(),charset:"utf8mb4".into(),tls:TlsMode::Disabled,list_all_databases:true,..Default::default()};
        let (session,events)=Session::start(profile,String::new(),false);
        let opened=match events.recv().await.unwrap(){Event::Connected(info)=>info.databases,_=>panic!("Connection failed")};
        assert!(opened.iter().any(|db|db=="mysql"));
        session.request(Operation{id:1,kind:OperationKind::Databases}).unwrap();
        let refreshed=match events.recv().await.unwrap(){Event::Operation{result:Ok(OperationResult::Databases(names)),..}=>names,_=>panic!("Database refresh failed")};
        assert_eq!(opened,refreshed);assert!(refreshed.iter().any(|db|db=="turbodbnote_script_fixture"));
        let mut id=1;
        for sql in ["SET NAMES utf8mb3","LOCK TABLES imported WRITE"]{
            id+=1;session.request(Operation{id,kind:OperationKind::Workbench(Box::new(Request{database:"turbodbnote_script_fixture".into(),action:Action::ScriptPrepare{sql:sql.into()}}))}).unwrap();
            let plan=match events.recv().await.unwrap(){Event::Operation{result:Ok(OperationResult::Workbench(reply)),..}=>match *reply{Reply::NotePrepared(plan)=>plan,_=>panic!("Unexpected preparation reply")},_=>panic!("Preparation failed")};
            id+=1;session.request(Operation{id,kind:OperationKind::Workbench(Box::new(Request{database:"turbodbnote_script_fixture".into(),action:Action::ScriptExecute{plan,confirmed:true}}))}).unwrap();
            assert!(matches!(events.recv().await.unwrap(),Event::Operation{result:Ok(_),..}));
        }
        id+=1;session.request(Operation{id,kind:OperationKind::Workbench(Box::new(Request{database:"turbodbnote_script_fixture".into(),action:Action::ScriptPrepare{sql:"SELECT @@character_set_client".into()}}))}).unwrap();
        let plan=match events.recv().await.unwrap(){Event::Operation{result:Ok(OperationResult::Workbench(reply)),..}=>match *reply{Reply::NotePrepared(plan)=>plan,_=>panic!("Unexpected read preparation")},_=>panic!("Read preparation failed")};
        id+=1;session.request(Operation{id,kind:OperationKind::Workbench(Box::new(Request{database:"turbodbnote_script_fixture".into(),action:Action::ScriptExecute{plan,confirmed:false}}))}).unwrap();
        match events.recv().await.unwrap(){Event::Operation{result:Ok(OperationResult::Workbench(reply)),..}=>match *reply{Reply::NoteResult(result)=>assert_eq!(result.rows.values[0][0].as_deref(),Some("utf8mb3")),_=>panic!("Unexpected read result")},_=>panic!("Read execution failed")};
        drop(session);
        tokio::time::timeout(Duration::from_secs(10),async{while let Ok(event)=events.recv().await{if matches!(event,Event::Disconnected){break;}}}).await.unwrap();
        sqlx::query("SET SESSION lock_wait_timeout=2").execute(&mut db).await.unwrap();
        sqlx::query("INSERT INTO imported VALUES(1003,'unlocked')").execute(&mut db).await.expect("Dropped script session retained its table lock");
        let mut document=Note::new("MySQL script fixture");document.cells.push(Cell{id:uuid::Uuid::new_v4(),title:String::new(),content:CellContent::SqlScript{script:imported.script.clone(),connection_id:None,database:None}});
        turbodbn_services::note_store::save(&path,&document,None).unwrap();
        println!("Isolated MySQL script controls, version comments, escaping, bulk INSERT, confirmation, target validation and lock cleanup passed");
    });
}
