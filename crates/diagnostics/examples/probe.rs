//! Safe local probe: expects an empty executable directory, never loads real SDKs.
use std::{path::PathBuf, time::Duration};
use turbodbn_diagnostics::{Event, config::Config};
fn main() {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("temporary directory required"),
    );
    let executable = directory.join("empty-sdk-dir");
    std::fs::create_dir_all(&executable).unwrap();
    assert!(
        !executable.join("diagnostics").exists(),
        "Probe must not load a real SDK"
    );
    let config = Config::from_json(
        br#"{"enabled":true,"app_id":"localtest","region":"global","record_usage":true}"#,
    )
    .unwrap();
    let guard = turbodbn_diagnostics::start(config, &executable, &directory, "test");
    turbodbn_diagnostics::track(Event::SettingsOpened);
    if std::env::args().any(|arg| arg == "--panic") {
        panic!("sensitive-test-payload-must-not-be-persisted");
    }
    std::thread::sleep(Duration::from_millis(100));
    drop(guard);
    println!("{}", turbodbn_diagnostics::status());
}
