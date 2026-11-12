//! Current-user login startup registration. Only changed by an explicit settings action.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    apply(enabled).map_err(|_| "settings.general.autostart_failed".into())
}

#[cfg(target_os = "windows")]
fn apply(enabled: bool) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let system = std::env::var_os("SystemRoot").ok_or(std::io::ErrorKind::NotFound)?;
    let mut command =
        std::process::Command::new(std::path::Path::new(&system).join("System32/reg.exe"));
    command.creation_flags(0x08000000);
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    if enabled {
        let executable = std::env::current_exe()?;
        let value = format!("\"{}\"", executable.display());
        command.args([
            "add",
            key,
            "/v",
            "TurboDBNote",
            "/t",
            "REG_SZ",
            "/d",
            &value,
            "/f",
        ]);
    } else {
        let status =
            std::process::Command::new(std::path::Path::new(&system).join("System32/reg.exe"))
                .creation_flags(0x08000000)
                .args(["query", key, "/v", "TurboDBNote"])
                .output()?;
        if !status.status.success() {
            return Ok(());
        }
        command.args(["delete", key, "/v", "TurboDBNote", "/f"]);
    }
    if command.output()?.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("startup registration failed"))
    }
}

#[cfg(target_os = "linux")]
fn apply(enabled: bool) -> std::io::Result<()> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .ok_or(std::io::ErrorKind::NotFound)?;
    let path = config.join("autostart/turbodbnote.desktop");
    if enabled {
        let executable = std::env::current_exe()?;
        let executable = executable.to_str().ok_or(std::io::ErrorKind::InvalidData)?;
        let escaped = executable
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%");
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(
            path,
            format!(
                "[Desktop Entry]\nType=Application\nName=TurboDBNote\nExec=\"{escaped}\"\nTerminal=false\n"
            ),
        )
    } else {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn apply(_: bool) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}
