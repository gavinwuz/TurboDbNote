//! Run only against an isolated local fixture; no real credentials or configuration files.
use turbodbn_services::{
    DatabaseKind,
    database::{
        self, Operation, OperationKind, OperationResult,
        workbench::{Action, Object, Reply, Request},
    },
    mysql::{Driver, Event, Profile, Session, TlsMode},
};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let profile = Profile {
        port: args[0].parse().unwrap(),
        database: "source_db".into(),
        driver: Driver::Legacy,
        legacy_library: args[1].clone(),
        charset: "server-default".into(),
        tls: TlsMode::Disabled,
        allow_old_auth: true,
        timeout_seconds: 30,
        ..database::defaults(DatabaseKind::MySql)
    };
    let (session, events) = Session::start(profile, String::new(), false);
    let version = match events.recv_blocking().unwrap() {
        Event::Connected(info) => info.version,
        _ => panic!("fixture connection failed"),
    };
    let mut id = 0;
    let mut call = |kind| {
        id += 1;
        session.request(Operation { id, kind }).unwrap();
        loop {
            match events.recv_blocking().unwrap() {
                Event::Operation { id: reply, result } if reply == id => break result,
                Event::Failed(_) => panic!("fixture session failed"),
                _ => {}
            }
        }
    };
    let created = call(OperationKind::CreateDatabase {
        name: "charset_copy".into(),
        charset: "gbk".into(),
        collation: String::new(),
    });
    if version.starts_with("4.0") {
        assert!(created.is_err());
    } else {
        assert!(created.is_ok());
    }
    for (name, data) in [("schema_copy", false), ("full_copy", true)] {
        let result = call(OperationKind::CopyDatabase {
            source: "source_db".into(),
            target: name.into(),
            charset: String::new(),
            include_data: data,
        })
        .unwrap();
        assert!(matches!(result, OperationResult::Copied { tables: 2, .. }));
        for table in ["parent", "child"] {
            let object = Object {
                schema: String::new(),
                name: table.into(),
                kind: "table".into(),
                comment: String::new(),
                parent: String::new(),
            };
            let result = call(OperationKind::Workbench(Box::new(Request {
                database: name.into(),
                action: Action::Page {
                    object: object.clone(),
                    offset: 0,
                    page_size: 10,
                },
            })))
            .unwrap();
            let OperationResult::Workbench(reply) = result else {
                panic!()
            };
            let Reply::Page { rows, .. } = *reply else {
                panic!()
            };
            assert_eq!(rows.values.len(), usize::from(data));
            let result = call(OperationKind::Workbench(Box::new(Request {
                database: name.into(),
                action: Action::Describe(object),
            })))
            .unwrap();
            let OperationResult::Workbench(reply) = result else {
                panic!()
            };
            let Reply::Schema(schema) = *reply else {
                panic!()
            };
            if table == "child" {
                assert!(schema.ddl.contains("FOREIGN KEY"));
                assert!(!schema.ddl.contains("`source_db`"));
            }
        }
        assert!(
            call(OperationKind::CopyDatabase {
                source: "source_db".into(),
                target: name.into(),
                charset: String::new(),
                include_data: data
            })
            .is_err()
        );
    }
    println!("PASS: copy schema/data, foreign keys and no-overwrite");
}
