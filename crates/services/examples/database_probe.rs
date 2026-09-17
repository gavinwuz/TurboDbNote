//! Isolated test servers only; explicit kind/port/database and optional MySQL DLL.
use turbodbn_services::{
    DatabaseKind,
    database::{self, Operation, OperationKind, OperationResult},
    mysql::{Driver, Event, Profile, Session, TlsMode},
};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let kind = match args[0].as_str() {
        "pg" => DatabaseKind::PostgreSql,
        "redis" => DatabaseKind::Redis,
        "clickhouse" => DatabaseKind::ClickHouse,
        _ => DatabaseKind::MySql,
    };
    let database = args.get(2).cloned().unwrap_or_default();
    let profile = Profile {
        port: args[1].parse().unwrap(),
        database: database.clone(),
        tls: TlsMode::Disabled,
        timeout_seconds: 10,
        legacy_library: args.get(3).cloned().unwrap_or_default(),
        driver: if args.get(3).is_some() {
            Driver::Legacy
        } else {
            Driver::Modern
        },
        charset: if args.get(3).is_some() {
            "server-default"
        } else {
            "utf8mb4"
        }
        .into(),
        allow_old_auth: true,
        ..database::defaults(kind)
    };
    let (session, events) = Session::start(profile, String::new(), false);
    match events.recv_blocking().unwrap() {
        Event::Connected(info) => println!("Connected {} {}", kind.label(), info.version),
        Event::Failed(e) => panic!("{e}"),
        _ => panic!("connection expected"),
    }
    session
        .request(Operation {
            id: 1,
            kind: OperationKind::Objects {
                database: database.clone(),
                cursor: None,
            },
        })
        .unwrap();
    let items = match events.recv_blocking().unwrap() {
        Event::Operation {
            id: 1,
            result: Ok(OperationResult::Objects { items, .. }),
        } => items,
        Event::Operation { result: Err(e), .. } => panic!("{e}"),
        _ => panic!("objects expected"),
    };
    assert!(!items.is_empty(), "no fixture objects");
    session
        .request(Operation {
            id: 2,
            kind: OperationKind::Details {
                database: database.clone(),
                object: items[0].clone(),
            },
        })
        .unwrap();
    match events.recv_blocking().unwrap() {
        Event::Operation {
            id: 2,
            result: Ok(OperationResult::Details(rows)),
        } => assert!(!rows.is_empty()),
        Event::Operation { result: Err(e), .. } => panic!("{e}"),
        _ => panic!("details expected"),
    }
    if kind.can_create_database() {
        let name = format!("probe_{}", uuid::Uuid::new_v4().simple());
        session
            .request(Operation {
                id: 3,
                kind: OperationKind::CreateDatabase { name: name.clone() },
            })
            .unwrap();
        match events.recv_blocking().unwrap() {
            Event::Operation {
                id: 3,
                result: Ok(OperationResult::Created(created)),
            } => assert_eq!(created, name),
            Event::Operation { result: Err(e), .. } => panic!("{e}"),
            _ => panic!("create expected"),
        }
    }
    drop(session);
    assert!(matches!(
        events.recv_blocking().unwrap(),
        Event::Disconnected
    ));
    println!("PASS: objects, details, create capability, disconnect");
}
