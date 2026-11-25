//! Run only against the caller-owned raw_gbk_edit_review fixture; never initializes a server.
use turbodbn_services::{
    DatabaseKind,
    database::{
        Operation, OperationKind, OperationResult,
        workbench::{Action, Change, Object, PageOptions, Reply},
    },
    mysql::{Driver, Event, Profile, Session, TlsMode},
};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let port = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .expect("owned fixture port required");
    let profile = Profile {
        kind: DatabaseKind::MySql,
        host: "127.0.0.1".into(),
        port,
        user: "root".into(),
        database: "raw_gbk_edit_review".into(),
        charset: "gbk".into(),
        mysql_raw_gbk: true,
        driver: Driver::Legacy,
        legacy_library: args
            .get(2)
            .expect("verified legacy DLL path required")
            .clone(),
        allow_old_auth: true,
        tls: TlsMode::Disabled,
        timeout_seconds: 5,
        ..Default::default()
    };
    let (session, events) = Session::start(profile, String::new(), false);
    loop {
        match events.recv_blocking().expect("fixture session closed") {
            Event::Connected(_) => break,
            Event::Failed(_) => panic!("fixture connection failed"),
            _ => {}
        }
    }
    let mut sequence = 0;
    let mut call = |action| {
        sequence += 1;
        session
            .request(Operation {
                id: sequence,
                kind: OperationKind::Workbench(Box::new(
                    turbodbn_services::database::workbench::Request {
                        database: "raw_gbk_edit_review".into(),
                        action,
                    },
                )),
            })
            .unwrap();
        loop {
            match events.recv_blocking().unwrap() {
                Event::Operation { id, result } if id == sequence => {
                    return result.and_then(|r| {
                        if let OperationResult::Workbench(reply) = r {
                            Ok(*reply)
                        } else {
                            Err("unexpected response".into())
                        }
                    });
                }
                Event::Failed(_) => panic!("fixture transport failed"),
                _ => {}
            }
        }
    };
    let object = Object {
        schema: String::new(),
        name: "items".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    let Reply::GridPage { rows, schema, .. } = call(Action::GridPage {
        object: object.clone(),
        options: PageOptions::default(),
    })
    .expect("fixture read failed") else {
        panic!("wrong page")
    };
    let id = rows.columns.iter().position(|c| c == "id").unwrap();
    let name = rows.columns.iter().position(|c| c == "name").unwrap();
    let original = rows
        .values
        .iter()
        .find(|r| r[id].as_deref() == Some("1"))
        .unwrap()
        .clone();
    assert_eq!(original[name].as_deref(), Some("张"));
    assert!(schema.raw_gbk_columns.contains("name"));
    let changes = vec![Change {
        original: Some(original.clone()),
        values: Some(
            [
                ("name".into(), Some("李'\\".into())),
                ("label".into(), Some("UTF8中文".into())),
            ]
            .into_iter()
            .collect(),
        ),
    }];
    assert!(
        call(Action::Commit {
            object: object.clone(),
            ddl: schema.ddl.clone(),
            changes
        })
        .is_ok(),
        "raw grid commit failed"
    );
    let stale = vec![Change {
        original: Some(original),
        values: Some(
            [("name".into(), Some("不应覆盖".into()))]
                .into_iter()
                .collect(),
        ),
    }];
    assert!(
        call(Action::Commit {
            object: object.clone(),
            ddl: schema.ddl.clone(),
            changes: stale
        })
        .is_err(),
        "stale original must fail"
    );
    let Reply::GridPage { rows, .. } = call(Action::GridPage {
        object: object.clone(),
        options: PageOptions::default(),
    })
    .unwrap() else {
        panic!("wrong page")
    };
    let current = rows
        .values
        .iter()
        .find(|r| r[id].as_deref() == Some("1"))
        .unwrap()
        .clone();
    assert_eq!(current[name].as_deref(), Some("李'\\"));
    let invalid = vec![Change {
        original: Some(current),
        values: Some([("name".into(), Some("😀".into()))].into_iter().collect()),
    }];
    assert!(
        call(Action::Commit {
            object,
            ddl: schema.ddl,
            changes: invalid
        })
        .is_err(),
        "unencodable text must fail"
    );
    assert!(
        call(Action::Execute {
            sql: "DELETE FROM items".into(),
            guard: None
        })
        .is_err(),
        "arbitrary writes remain blocked"
    );
    let Reply::NotePrepared(plan) = call(Action::NotePrepare {
        sql: "SELECT * FROM raw_gbk_edit_review.items a WHERE a.id=1".into(),
    })
    .expect("note prepare failed") else {
        panic!("wrong note plan")
    };
    let Reply::NoteResult(result) = call(Action::NoteExecute {
        plan: plan.clone(),
        confirmed: false,
        offset: 0,
        page_size: 20,
    })
    .expect("note read failed") else {
        panic!("wrong note result")
    };
    let metadata = result.grid.as_ref().expect("missing note metadata");
    assert!(metadata.editable);
    assert!(metadata.schema.raw_gbk_columns.contains("name"));
    let change = Change {
        original: Some(result.rows.values[0].clone()),
        values: Some([("name".into(), Some("张三".into()))].into_iter().collect()),
    };
    let action = turbodbn_services::database::notebook_query::GridAction::Commit {
        columns: result.rows.columns.clone(),
        ddl: metadata.schema.ddl.clone(),
        changes: vec![change],
    };
    assert!(
        call(Action::NoteGrid {
            plan: plan.clone(),
            confirmed: false,
            action: action.clone()
        })
        .is_err(),
        "unconfirmed Note commit must fail"
    );
    assert!(
        call(Action::NoteGrid {
            plan,
            confirmed: true,
            action
        })
        .is_ok(),
        "confirmed Note grid commit failed"
    );
    let Reply::GridPage { rows, schema, .. } = call(Action::GridPage {
        object: Object {
            schema: String::new(),
            name: "items".into(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        },
        options: PageOptions::default(),
    })
    .unwrap() else {
        panic!("wrong page")
    };
    let first = rows
        .values
        .iter()
        .find(|row| row[id].as_deref() == Some("1"))
        .unwrap()
        .clone();
    let mut second = rows
        .values
        .iter()
        .find(|row| row[id].as_deref() == Some("2"))
        .unwrap()
        .clone();
    second[name] = Some("stale original".into());
    let updates = vec![
        Change {
            original: Some(first.clone()),
            values: Some([("name".into(), Some("临时".into()))].into_iter().collect()),
        },
        Change {
            original: Some(second),
            values: Some(
                [("name".into(), Some("不应提交".into()))]
                    .into_iter()
                    .collect(),
            ),
        },
    ];
    let object = Object {
        schema: String::new(),
        name: "items".into(),
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    };
    assert!(
        call(Action::Commit {
            object: object.clone(),
            ddl: schema.ddl,
            changes: updates
        })
        .is_err()
    );
    let Reply::GridPage { rows, .. } = call(Action::GridPage {
        object,
        options: PageOptions::default(),
    })
    .unwrap() else {
        panic!("wrong page")
    };
    assert_eq!(
        rows.values
            .iter()
            .find(|row| row[id].as_deref() == Some("1"))
            .unwrap(),
        &first,
        "earlier successful update must roll back on later conflict"
    );
    println!(
        "PASS: raw GBK transactional grid commit, Unicode column preservation, stale-row rollback, unencodable rejection and arbitrary-write restriction"
    );
}
