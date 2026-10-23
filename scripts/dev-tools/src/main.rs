mod package;
mod release;
use regex::Regex;
use std::{
    collections::VecDeque,
    env,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::OnceLock,
};

fn clean(line: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    static SECRET: OnceLock<Regex> = OnceLock::new();
    let line = ANSI
        .get_or_init(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap())
        .replace_all(line, "");
    if SECRET.get_or_init(|| Regex::new(r"(?i)(password|secret|\bkey\b|api[_-]?key|access[_-]?token|authorization)\s*[:=]|^\s*(left|right):|://[^\s/]*@").unwrap()).is_match(&line) {
        "[sensitive diagnostic value omitted]".into()
    } else { line.into_owned() }
}
fn annotation(line: &str, title: &str) {
    let escaped = line
        .chars()
        .take(700)
        .collect::<String>()
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    println!("::error title={title}::{escaped}");
}
fn cargo(args: &[String]) -> Result<i32, String> {
    let mut process = Command::new("cargo")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "Cannot start Cargo")?;
    let stdout = process.stdout.take().unwrap();
    let stderr = process.stderr.take().unwrap();
    let forward = |stream: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let line = clean(&line.unwrap_or_else(|_| "Unreadable Cargo diagnostic".into()));
                println!("{line}");
                if line.starts_with("error")
                    || line.contains("panicked at ")
                    || (line.starts_with("test ") && line.ends_with("FAILED"))
                {
                    annotation(&line, "Cargo diagnostic");
                }
            }
        })
    };
    let a = forward(Box::new(stdout));
    let b = forward(Box::new(stderr));
    let status = process.wait().map_err(|_| "Cannot wait for Cargo")?;
    a.join().map_err(|_| "Cargo output failed")?;
    b.join().map_err(|_| "Cargo output failed")?;
    Ok(status.code().unwrap_or(1))
}
fn diagnostics() -> Result<(), String> {
    let job = env::var("JOB_ID").unwrap_or_default();
    let job = if job.is_empty() {
        let path = env::var("GITHUB_EVENT_PATH").map_err(|_| "Missing event path")?;
        let event: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read workflow event")?)
                .map_err(|_| "Invalid workflow event")?;
        event["head_commit"]["message"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .find_map(|line| line.strip_prefix("Diagnose-Job: "))
            .unwrap_or_default()
            .to_owned()
    } else {
        job
    };
    let repo = env::var("GITHUB_REPOSITORY").map_err(|_| "Missing repository")?;
    if job.is_empty()
        || !job.bytes().all(|c| c.is_ascii_digit())
        || !Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
            .unwrap()
            .is_match(&repo)
    {
        return Err("Invalid job or repository identifier".into());
    }
    let mut process = Command::new("gh")
        .args(["api", &format!("repos/{repo}/actions/jobs/{job}/logs")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot start GitHub CLI")?;
    let mut lines = VecDeque::new();
    for line in BufReader::new(process.stdout.take().unwrap()).lines() {
        let line = clean(&line.map_err(|_| "Cannot read job logs")?);
        let lower = line.to_lowercase();
        if [
            "password",
            "secret",
            "token",
            "api_key",
            "authorization",
            "://",
        ]
        .iter()
        .any(|s| lower.contains(s))
        {
            continue;
        }
        if [
            "error",
            "panic",
            "fatal",
            "traceback",
            "no space",
            "killed",
            "not found",
            "verification failed",
        ]
        .iter()
        .any(|s| lower.contains(s))
        {
            lines.push_back(line.chars().take(700).collect::<String>());
            if lines.len() > 35 {
                lines.pop_front();
            }
        }
    }
    if !process
        .wait()
        .map_err(|_| "Cannot wait for GitHub CLI")?
        .success()
    {
        return Err("Job log request failed; response details omitted".into());
    }
    for line in lines {
        annotation(&line, "Captured build diagnostic");
    }
    Ok(())
}
fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = match args.first().map(String::as_str) {
        Some("prefetch-clients") if args.len() == 2 || args.len() == 4 => package::prefetch(&root, &args[1], args.get(2).filter(|v| v.as_str() != "-").map(std::path::Path::new), args.get(3).filter(|v| v.as_str() != "-").map(std::path::Path::new)).map(|_|0),
        Some("run-cargo") => cargo(&args[1..]),
        Some("version") => release::version(&root).map(|v| {println!("{v}"); 0}),
        Some("release") => release::validate_cli(&root, &args[1..]).map(|_| 0),
        Some("build-diagnostics") => diagnostics().map(|_| 0),
        Some("prepare-db2") if args.len() == 3 || args.len() == 4 => turbodbn_services::db2_bundle::prepare(std::path::Path::new(&args[1]), std::path::Path::new(&args[2]), args.get(3).map(std::path::Path::new), &std::sync::atomic::AtomicBool::new(false)).map(|_|0),
        Some("prepare-oracle") if args.len() == 4 || args.len() == 5 => turbodbn_services::oracle_bundle::prepare(std::path::Path::new(&args[1]), std::path::Path::new(&args[2]), &args[3], args.get(4).map(std::path::Path::new), &std::sync::atomic::AtomicBool::new(false)).map(|_|0),
        Some("driver-components") if args.len() == 5 => turbodbn_services::driver_packages::build(std::path::Path::new(&args[1]), std::path::Path::new(&args[2]), &args[3], &args[4]).map(|_| 0),
        Some("package-linux") => package::linux(&root).map(|_| 0),
        Some("prepare-release") => release::prepare(&root, &args[1..]).map(|_| 0),
        _ => Err("Usage: turbodbnote-tools version | release --version V [--notes FILE] [--publication] | prepare-release VERSION | driver-components NATIVE-DIR ZIP VERSION TARGET | prefetch-clients TARGET [ORACLE-ZIP|- DB2-ZIP|-] | package-linux | run-cargo ARGS | build-diagnostics".into()),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{}", clean(&error));
            std::process::exit(1)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_redact_before_output() {
        assert_eq!(
            clean("\x1b[31merror[E0425]: unknown item\x1b[0m"),
            "error[E0425]: unknown item"
        );
        for line in [
            "authorization: private",
            "left: private",
            "error: mysql://user:private@host",
        ] {
            assert_eq!(clean(line), "[sensitive diagnostic value omitted]");
        }
    }
}
