//! The caller owns the isolated temporary server and passes its port.
use std::sync::Arc;
use turbodbn_services::database::{
    Operation, OperationKind, OperationResult,
    notebook_query::Plan,
    workbench::{Action, Reply, Request},
};
use turbodbn_services::mysql::{Event, Session};
use turbodbn_services::{
    DatabaseKind,
    database::transfer::{self, Contents, Control, Endpoint, Job, Mode, Options, ReadPermission},
    mysql::{Profile, TlsMode},
};
fn main() -> Result<(), String> {
    let port = std::env::args()
        .nth(1)
        .ok_or("port required")?
        .parse()
        .map_err(|_| "invalid port")?;
    let endpoint = Endpoint {
        profile: Profile {
            kind: DatabaseKind::MySql,
            name: "permission fixture".into(),
            host: "127.0.0.1".into(),
            port,
            user: "fixture_reader".into(),
            database: "permission_fixture".into(),
            target_version: "8.0.32".into(),
            tls: TlsMode::Disabled,
            ..Default::default()
        },
        password: String::new(),
    };
    let control = Control::default();
    let (catalog, _) = transfer::catalog(&endpoint, &control)?;
    let permissions = transfer::read_permissions(&endpoint, &catalog, &control);
    if permissions.0.get("view") != Some(&ReadPermission::Denied) || !permissions.denied("event") {
        return Err("denied view/event check failed".into());
    }
    let directory = tempfile::tempdir().map_err(|_| "temporary directory failed")?;
    let file = directory.path().join("tables.sql");
    let (sender, _receiver) = async_channel::bounded(64);
    Job {
        mode: Mode::Export,
        source: endpoint,
        target: None,
        database: String::new(),
        file: file.clone(),
        contents: Contents {
            schema: true,
            data: true,
        },
        options: Options {
            views: false,
            triggers: false,
            routines: false,
            events: false,
            ..Default::default()
        },
    }
    .run(Arc::new(control), &sender)?;
    if !file.is_file() {
        return Err("export file missing".into());
    }
    let (session, events) = Session::start(
        Profile {
            port,
            user: "root".into(),
            database: "permission_fixture".into(),
            tls: TlsMode::Disabled,
            ..Default::default()
        },
        String::new(),
        false,
    );
    loop {
        match events
            .recv_blocking()
            .map_err(|_| "fixture session ended")?
        {
            Event::Connected(_) => break,
            Event::Failed(_) => return Err("fixture root connection failed".into()),
            _ => {}
        }
    }
    let mut sequence = 0;
    let mut call = |kind| {
        sequence += 1;
        session
            .request(Operation { id: sequence, kind })
            .map_err(|_| "fixture request failed".to_string())?;
        loop {
            match events
                .recv_blocking()
                .map_err(|_| "fixture session ended".to_string())?
            {
                Event::Operation { id, result } if id == sequence => break result,
                Event::Failed(_) => break Err("fixture session failed".into()),
                _ => {}
            }
        }
    };
    call(OperationKind::CreateDatabase {
        name: "collation_fixture".into(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_general_ci".into(),
    })?;
    if call(OperationKind::CreateDatabase {
        name: "collation_mismatch".into(),
        charset: "latin1".into(),
        collation: "utf8mb4_general_ci".into(),
    })
    .is_ok()
    {
        return Err("mismatched collation was accepted".into());
    }
    let sql = "SELECT DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME='collation_fixture'";
    let result = call(OperationKind::Workbench(Box::new(Request {
        database: "permission_fixture".into(),
        action: Action::NoteExecute {
            plan: Plan {
                sql: sql.into(),
                read_only: true,
                confirm: false,
                guards: vec![],
            },
            confirmed: false,
            offset: 0,
            page_size: 20,
        },
    })))?;
    let OperationResult::Workbench(reply) = result else {
        return Err("fixture metadata reply missing".into());
    };
    let Reply::NoteResult(page) = *reply else {
        return Err("fixture metadata page missing".into());
    };
    if page
        .rows
        .values
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_deref())
        != Some("utf8mb4_general_ci")
    {
        return Err("created collation differs".into());
    }
    println!("PASS: permissions, table export, database collation and mismatch rejection");
    Ok(())
}
