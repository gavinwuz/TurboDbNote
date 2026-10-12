//! External adapters live in a disposable process. No foreign runtime enters the GUI.
use crate::mysql::{Event, Profile, Session};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub(crate) struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}
pub(crate) fn acquire() -> Result<Permit, String> {
    ACTIVE
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < 8).then_some(n + 1)
        })
        .map(|_| Permit)
        .map_err(|_| "drivers.session_limit".into())
}

pub fn directory() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut base = exe.parent()?.to_path_buf();
    if base
        .file_name()
        .is_some_and(|s| s == "deps" || s == "examples")
    {
        base.pop();
    }
    Some(base.join("drivers/native"))
}
pub fn installed() -> bool {
    directory().is_some_and(|p| p.join("turbodbnote-driver-host.exe").is_file())
}
pub fn status_key() -> &'static str {
    if installed() {
        "drivers.on_demand"
    } else {
        "drivers.runtime_missing"
    }
}
pub(crate) fn command(p: &Profile) -> Result<tokio::process::Command, String> {
    let spec = p
        .driver_spec()
        .filter(|d| d.kind == crate::DatabaseKind::External)
        .ok_or("drivers.invalid_identity")?;
    if spec.component == "removed" {
        return Err("drivers.removed".into());
    }
    let dir = directory().ok_or("drivers.runtime_missing")?;
    let executable = dir.join("turbodbnote-driver-host.exe");
    if !executable.is_file() {
        return Err("drivers.runtime_missing".into());
    }
    let mut command = tokio::process::Command::new(executable);
    command.current_dir(dir);
    Ok(command)
}
pub fn start(
    p: Profile,
    password: String,
    test: bool,
) -> (Session, async_channel::Receiver<Event>) {
    crate::mysql_legacy::start(p, password, test)
}

#[cfg(test)]
mod tests {
    #[test]
    fn retired_h2_records_load_but_cannot_start() {
        assert!(!crate::driver_catalog::DRIVERS.iter().any(|d| d.id == "h2"));
        let old = crate::driver_catalog::find("h2").unwrap().profile();
        let restored: crate::mysql::Profile =
            serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.driver_label(), "H2");
        assert_eq!(super::command(&restored).err().unwrap(), "drivers.removed");
    }
    #[test]
    fn worker_budget_is_released_when_sessions_end() {
        let permits = (0..8)
            .map(|_| super::acquire().unwrap())
            .collect::<Vec<_>>();
        assert!(super::acquire().is_err());
        drop(permits);
        assert!(super::acquire().is_ok());
    }
}
