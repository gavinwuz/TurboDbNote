//! Validate only an explicitly supplied, disposable Access fixture seeded with sample_rows(id,label).
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Worker {
    fn send(&mut self, value: Value) -> Result<Value, &'static str> {
        let bytes = serde_json::to_vec(&value).map_err(|_| "Encode failed")?;
        self.input
            .write_all(&bytes)
            .and_then(|_| self.input.write_all(b"\n"))
            .and_then(|_| self.input.flush())
            .map_err(|_| "Input failed")?;
        let mut line = String::new();
        self.output
            .by_ref()
            .take(1048577)
            .read_line(&mut line)
            .map_err(|_| "Output failed")?;
        if line.len() > 1048576 {
            return Err("Response exceeded limit");
        }
        serde_json::from_str(&line).map_err(|_| "Invalid response")
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Supply an owned fixture.accdb path")?,
    );
    if !fixture.is_absolute() || !fixture.is_file() {
        return Err("Missing absolute fixture path".into());
    }
    let host = std::env::current_exe()?
        .parent()
        .and_then(|p| p.parent())
        .ok_or("Missing worker directory")?
        .join(if cfg!(windows) {
            "turbodbnote-driver-host.exe"
        } else {
            "turbodbnote-driver-host"
        });
    for driver in ["access", "odbc"] {
        let mut child = Command::new(&host)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Cannot start worker")?;
        let mut worker = Worker {
            input: child.stdin.take().ok_or("Missing stdin")?,
            output: BufReader::new(child.stdout.take().ok_or("Missing stdout")?),
            child,
        };
        let profile = json!({"driver_id":driver,"host":"localhost","port":1,"user":"","database":"","sqlite_path":fixture,"tls":"Disabled","timeout_seconds":10,"odbc":{"dsn":"","driver":"Microsoft Access Driver (*.mdb, *.accdb)","parameters":format!("DBQ={}",fixture.display())}});
        let info = worker.send(json!({"profile":profile,"password":""}))?;
        let database = info["info"]["database"]
            .as_str()
            .ok_or("Connection failed")?
            .to_string();
        let catalog = worker.send(json!({"kind":{"Objects":{"database":database}}}))?;
        let object = catalog["Ok"]["Objects"]["items"]
            .as_array()
            .ok_or("Catalog failed")?
            .iter()
            .find(|o| o["name"] == "sample_rows")
            .ok_or("Fixture table missing")?
            .clone();
        let schema = worker.send(
            json!({"kind":{"Workbench":{"database":database,"action":{"Describe":object}}}}),
        )?;
        assert_eq!(
            schema["Ok"]["Workbench"]["Schema"]["columns"]
                .as_array()
                .ok_or("Describe failed")?
                .len(),
            2
        );
        for offset in if driver == "odbc" {
            vec![0, 3]
        } else {
            vec![0]
        } {
            let page = worker.send(json!({"kind":{"Workbench":{"database":database,"action":{"GridPage":{"object":object,"options":{"page_size":3,"offset":offset}}}}}}))?;
            let rows = page["Ok"]["Workbench"]["GridPage"]["rows"]["values"]
                .as_array()
                .ok_or("Page failed")?;
            assert_eq!(rows.len(), 3);
            assert_eq!(
                rows[0][0].as_str().ok_or("Missing value")?,
                if offset == 0 { "1" } else { "4" }
            );
        }
        let denied = worker.send(json!({"kind":{"Workbench":{"database":database,"action":{"NotePrepare":{"sql":"DELETE FROM sample_rows"}}}}}))?;
        assert!(denied["Err"].is_string(), "Write action must be rejected");
        println!("{driver}: connection, catalog, fields, bounded pages and write rejection passed");
    }
    Ok(())
}
