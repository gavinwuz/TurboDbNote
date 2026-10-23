//! Explicit loopback fixture; never connects to a user database.
use sqlx::{
    ConnectOptions, Connection,
    mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode},
};
use turbodbn_services::{
    database::{
        Operation, OperationKind, OperationResult,
        workbench::{Action, Object, Reply, Request},
    },
    mysql::{Event, Profile, Session, TlsMode},
};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args.get(2).map(String::as_str), Some("--isolated"));
    let port = args[0].parse().unwrap();
    let library = args[1].clone();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
  let options=MySqlConnectOptions::new().host("127.0.0.1").port(port).username("root").ssl_mode(MySqlSslMode::Disabled).disable_statement_logging();
  let mut c=MySqlConnection::connect_with(&options).await.unwrap();
  if args.get(3).map(String::as_str) != Some("--existing-fixture") {
  sqlx::raw_sql("CREATE DATABASE gbk_read_fixture; CREATE USER 'gbk_read_fixture'@'127.0.0.1' IDENTIFIED BY ''; GRANT SELECT, SHOW VIEW ON gbk_read_fixture.* TO 'gbk_read_fixture'@'127.0.0.1'; CREATE TABLE gbk_read_fixture.cq_itemtype (id INT PRIMARY KEY,name VARCHAR(15) CHARACTER SET latin1 COLLATE latin1_bin NOT NULL,real_name VARCHAR(32) CHARACTER SET latin1 NOT NULL,payload VARBINARY(16),unicode_name VARCHAR(32) CHARACTER SET utf8) ENGINE=MyISAM DEFAULT CHARSET=latin1; INSERT INTO gbk_read_fixture.cq_itemtype VALUES(1,_latin1 0xB6F1B6BEC8FDBDC7,_latin1 0xD2ECBDE7CAA5CDBD,0x0081FF,'正常中文');").execute(&mut c).await.unwrap();
  }
  let profile=Profile {name:"Owned GBK verification".into(),user:"gbk_read_fixture".into(),port,database:"gbk_read_fixture".into(),charset:"gbk".into(),mysql_raw_gbk:true,legacy_library:library,tls:TlsMode::Disabled,allow_old_auth:true,..Default::default()};
  let (session,events)=Session::start(profile,String::new(),false);
  assert!(matches!(events.recv_blocking().unwrap(),Event::Connected(_)));
  let object=Object {schema:String::new(),name:"cq_itemtype".into(),kind:"table".into(),comment:String::new(),parent:String::new()};
  let actions=vec![Action::Page{object:object.clone(),offset:0,page_size:20},Action::GridPage{object:object.clone(),options:Default::default()},Action::Describe(object.clone()),Action::Page{object,offset:0,page_size:20}];
  for (id,action) in actions.into_iter().enumerate() {
   session.request(Operation{id:id as u64,kind:OperationKind::Workbench(Box::new(Request{database:"gbk_read_fixture".into(),action}))}).unwrap();
   match events.recv_blocking().unwrap() {
    Event::Operation{result:Ok(OperationResult::Workbench(reply)),..}=>match *reply {
     Reply::Page{rows,..}=>{assert_eq!(rows.values[0][1].as_deref(),Some("恶毒三角"));assert_eq!(rows.values[0][2].as_deref(),Some("异界圣徒"));assert_eq!(rows.values[0][3].as_deref(),Some("0x0081ff"));assert_eq!(rows.values[0][4].as_deref(),Some("正常中文"));},
     Reply::GridPage{rows,keyset,..}=>{assert!(!keyset);assert_eq!(rows.values[0][1].as_deref(),Some("恶毒三角"));},
     Reply::Schema(schema)=>assert_eq!(schema.columns[1].name,"name"), _=>panic!("Unexpected result"),
    },_=>panic!("Owned GBK worker failed"),
   }
  }
  session.request(Operation{id:10,kind:OperationKind::Workbench(Box::new(Request{database:"gbk_read_fixture".into(),action:Action::NotePrepare{sql:"SELECT name,payload FROM cq_itemtype".into()}}))}).unwrap();
  let plan = match events.recv_blocking().unwrap() {Event::Operation{result:Ok(OperationResult::Workbench(reply)),..}=>match *reply {Reply::NotePrepared(plan)=>plan,_=>panic!("Unexpected prepare")},_=>panic!("Prepare failed")};
  session.request(Operation{id:11,kind:OperationKind::Workbench(Box::new(Request{database:"gbk_read_fixture".into(),action:Action::NoteExecute{plan,confirmed:false,offset:0,page_size:20}}))}).unwrap();
  match events.recv_blocking().unwrap() {Event::Operation{result:Ok(OperationResult::Workbench(reply)),..}=>match *reply {Reply::NoteResult(result)=>{assert_eq!(result.rows.values[0][0].as_deref(),Some("恶毒三角"));assert_eq!(result.rows.values[0][1].as_deref(),Some("0x0081ff"));},_=>panic!("Unexpected query reply")},_=>panic!("Read query failed")}
  session.request(Operation{id:9,kind:OperationKind::Workbench(Box::new(Request{database:"gbk_read_fixture".into(),action:Action::Execute{sql:"DELETE FROM cq_itemtype".into(),guard:None}}))}).unwrap();
  assert!(matches!(events.recv_blocking().unwrap(),Event::Operation{result:Err(error),..} if error=="mysql.gbk.read_only"));
  drop(session); c.close().await.unwrap();
  println!("PASS: GBK latin1_bin, Latin1, binary, UTF8, metadata restoration and rejected writes on isolated MySQL");
 });
}
