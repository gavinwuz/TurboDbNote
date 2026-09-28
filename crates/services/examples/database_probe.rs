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
                kind: OperationKind::CreateDatabase {
                    name: name.clone(),
                    charset: String::new(),
                },
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
    if matches!(kind, DatabaseKind::MySql | DatabaseKind::PostgreSql) {
        workbench_probe(&session, &events, kind, &database);
    }
    if kind == DatabaseKind::ClickHouse {
        workbench_http_probe(&session, &events, &database);
    }
    drop(session);
    assert!(matches!(
        events.recv_blocking().unwrap(),
        Event::Disconnected
    ));
    println!("PASS: objects, details, create capability, disconnect");
}

fn workbench_probe(
    session: &Session,
    events: &async_channel::Receiver<Event>,
    kind: DatabaseKind,
    db: &str,
) {
    use turbodbn_services::database::workbench::{self, Action, Object, Reply, Request};
    let mut seq = 10;
    let mut call = |action| {
        seq += 1;
        session
            .request(Operation {
                id: seq,
                kind: OperationKind::Workbench(Box::new(Request {
                    database: db.into(),
                    action,
                })),
            })
            .unwrap();
        match events.recv_blocking().unwrap() {
            Event::Operation {
                id,
                result: Ok(OperationResult::Workbench(reply)),
            } if id == seq => *reply,
            Event::Operation { result: Err(e), .. } => panic!("workbench: {e}"),
            _ => panic!("workbench reply expected"),
        }
    };
    let object = Object {
        schema: if kind == DatabaseKind::PostgreSql {
            "public".into()
        } else {
            String::new()
        },
        name: format!("workbench_{}", uuid::Uuid::new_v4().simple()),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let qualified = workbench::qualified(kind, db, &object);
    call(Action::Execute {
        sql: format!(
            "CREATE TABLE {qualified} (id INTEGER PRIMARY KEY, name VARCHAR(80) DEFAULT 'kept');"
        ),
        guard: None,
    });
    call(Action::Execute {
        sql: format!(
            "INSERT INTO {qualified}(id,name) VALUES {}",
            (1..=205)
                .map(|i| format!("({i},{})", if i == 1 { "NULL" } else { "''" }))
                .collect::<Vec<_>>()
                .join(",")
        ),
        guard: None,
    });
    let Reply::Catalog { objects, .. } = call(Action::Catalog) else {
        panic!()
    };
    assert!(objects.iter().any(|o| o.name == object.name));
    let Reply::Schema(schema) = call(Action::Describe(object.clone())) else {
        panic!()
    };
    assert_eq!(schema.columns.len(), 2);
    assert!(schema.columns[0].primary);
    assert!(schema.ddl.contains("kept"));
    for (offset, count, more) in [(0, 100, true), (100, 100, true), (200, 5, false)] {
        let Reply::Page {
            rows, more: actual, ..
        } = call(Action::Page {
            object: object.clone(),
            offset,
            page_size: 100,
        })
        else {
            panic!()
        };
        assert_eq!(rows.values.len(), count);
        assert_eq!(actual, more);
        if offset == 0 {
            assert_eq!(rows.values[0][1], None);
            assert_eq!(rows.values[1][1], Some(String::new()));
        }
    }
    let sql = workbench::change_column(
        kind,
        db,
        &object,
        &schema.columns[1],
        &schema.ddl,
        "label",
        &schema.columns[1].data_type,
        &schema.columns[1].comment,
    )
    .unwrap();
    call(Action::Execute {
        sql,
        guard: Some((object.clone(), schema.ddl)),
    });
    let Reply::Schema(changed) = call(Action::Describe(object.clone())) else {
        panic!()
    };
    assert_eq!(changed.columns[1].name, "label");
    assert!(changed.ddl.contains("kept"));
    call(Action::Execute {
        sql: format!("DROP TABLE {qualified}"),
        guard: Some((object, changed.ddl)),
    });
    println!(
        "PASS: workbench catalog, schema, three pages, NULL, empty strings, guarded column rename and preserved default"
    );
}

fn workbench_http_probe(session: &Session, events: &async_channel::Receiver<Event>, db: &str) {
    use turbodbn_services::database::workbench::{Action, Reply, Request};
    let mut seq = 100;
    let mut call = |action| {
        seq += 1;
        session
            .request(Operation {
                id: seq,
                kind: OperationKind::Workbench(Box::new(Request {
                    database: db.into(),
                    action,
                })),
            })
            .unwrap();
        match events.recv_blocking().unwrap() {
            Event::Operation {
                id,
                result: Ok(OperationResult::Workbench(reply)),
            } if id == seq => *reply,
            Event::Operation { result: Err(e), .. } => panic!("workbench: {e}"),
            _ => panic!("workbench response expected"),
        }
    };
    let Reply::Catalog { objects, .. } = call(Action::Catalog) else {
        panic!()
    };
    let table = objects.into_iter().find(|o| o.kind == "table").unwrap();
    let Reply::Schema(schema) = call(Action::Describe(table.clone())) else {
        panic!()
    };
    assert!(!schema.columns.is_empty());
    assert!(!schema.ddl.is_empty());
    let Reply::Page { rows, .. } = call(Action::Page {
        object: table,
        offset: 0,
        page_size: 100,
    }) else {
        panic!()
    };
    assert_eq!(rows.columns.len(), schema.columns.len());
    assert!(!rows.values.is_empty());
    println!("PASS: ClickHouse workbench catalog, schema and bounded page response");
}
