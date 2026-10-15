//! Loopback-only timeout regression against an explicitly selected owned test server.
use turbodbn_services::{
    DatabaseKind,
    database::{
        Operation, OperationKind, OperationResult,
        workbench::{Action, Reply, Request},
    },
    mysql::{Driver, Event, Profile, Session, TlsMode},
};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args.get(1).map(String::as_str), Some("--isolated"));
    let port = args[0].parse().expect("Owned loopback port");
    for (inherit, execution, success) in [(true, 4, true), (true, 1, false), (false, 4, true)] {
        turbodbn_services::timeout_policy::configure(turbodbn_services::timeout_policy::Defaults {
            connect: 1,
            execution: if inherit { execution } else { 1 },
        })
        .unwrap();
        let p = Profile {
            kind: DatabaseKind::MySql,
            driver: Driver::Modern,
            host: "127.0.0.1".into(),
            port,
            database: "trigger_fixture".into(),
            tls: TlsMode::Disabled,
            timeout_seconds: 1,
            execution_timeout_seconds: if inherit { 1 } else { execution },
            inherit_timeouts: inherit,
            ..Default::default()
        };
        let (session, events) = Session::start(p, String::new(), false);
        turbodbn_services::timeout_policy::configure(turbodbn_services::timeout_policy::Defaults {
            connect: 1,
            execution: 1,
        })
        .unwrap();
        assert!(
            matches!(events.recv_blocking().unwrap(), Event::Connected(_)),
            "Owned fixture connection failed"
        );
        session
            .request(Operation {
                id: 1,
                kind: OperationKind::Workbench(Box::new(Request {
                    database: "trigger_fixture".into(),
                    action: Action::ScriptPrepare {
                        sql: "SELECT SLEEP(2)".into(),
                    },
                })),
            })
            .unwrap();
        let plan = match events.recv_blocking().unwrap() {
            Event::Operation {
                id: 1,
                result: Ok(OperationResult::Workbench(r)),
            } => match *r {
                Reply::NotePrepared(plan) => plan,
                _ => panic!("Unexpected prepare reply"),
            },
            _ => panic!("Prepare failed"),
        };
        session
            .request(Operation {
                id: 2,
                kind: OperationKind::Workbench(Box::new(Request {
                    database: "trigger_fixture".into(),
                    action: Action::ScriptExecute {
                        plan,
                        confirmed: true,
                    },
                })),
            })
            .unwrap();
        match events.recv_blocking().unwrap() {
            Event::Operation { id: 2, result } => {
                if success {
                    assert!(
                        matches!(result, Ok(OperationResult::Workbench(_))),
                        "Execution should exceed connect timeout successfully"
                    );
                } else {
                    assert_eq!(result.unwrap_err(), "script.error.timeout");
                }
            }
            _ => panic!("Unexpected execute reply"),
        }
        drop(session);
        println!(
            "connect=1s execution={execution}s inherited={inherit} expected_success={success}: passed"
        );
    }
}
