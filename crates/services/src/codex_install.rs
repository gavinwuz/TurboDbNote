//! Hash-pinned official native CLI installation. No shell, PATH edits or authentication.
use crate::{
    codex_discovery::Candidate,
    updates::{self, Asset, Progress},
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub const VERSION: &str = "0.163.0";
pub const GUIDE: &str = "https://developers.openai.com/codex/cli/";
const LIMIT: u64 = 600 * 1024 * 1024;
const ERROR: &str = "agents.setup.error.package";
struct Package {
    name: &'static str,
    binary: &'static str,
    hash: &'static str,
    size: u64,
    target: &'static str,
}
fn package() -> Result<Package, String> {
    if !cfg!(target_arch = "x86_64") {
        return Err("agents.setup.error.platform".into());
    }
    if cfg!(windows) {
        Ok(Package {
            name: "codex-x86_64-pc-windows-msvc.exe.zip",
            binary: "codex-x86_64-pc-windows-msvc.exe",
            hash: "dabdb4d16aaf10fea40b74811c96efddcd9138f122163b69ed604fcb3eb704c3",
            size: 167080091,
            target: "windows-x64",
        })
    } else if cfg!(target_os = "linux") {
        Ok(Package {
            name: "codex-x86_64-unknown-linux-musl.tar.gz",
            binary: "codex-x86_64-unknown-linux-musl",
            hash: "beb339e8ecd9ba057a6bb2dc81ad8ca474812edfbf0fc5056ca818797648dde4",
            size: 112589966,
            target: "linux-x64",
        })
    } else {
        Err("agents.setup.error.platform".into())
    }
}
pub fn executable_name() -> &'static str {
    if cfg!(windows) { "codex.exe" } else { "codex" }
}
pub fn managed_path(root: &Path) -> Result<PathBuf, String> {
    Ok(root
        .join(VERSION)
        .join(package()?.target)
        .join(executable_name()))
}
fn cancelled(flag: &AtomicBool) -> Result<(), String> {
    if flag.load(Ordering::Relaxed) {
        Err("agents.setup.cancelled".into())
    } else {
        Ok(())
    }
}
fn native_header(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| ERROR)?;
    let mut bytes = [0; 4096];
    let n = file.read(&mut bytes).map_err(|_| ERROR)?;
    if cfg!(windows) {
        if n < 64 || &bytes[..2] != b"MZ" {
            return Err(ERROR.into());
        }
        let offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        if offset.checked_add(6).is_none_or(|end| end > n)
            || &bytes[offset..offset + 4] != b"PE\0\0"
            || bytes[offset + 4..offset + 6] != [0x64, 0x86]
        {
            return Err(ERROR.into());
        }
    } else if n < 20
        || &bytes[..4] != b"\x7fELF"
        || bytes[4] != 2
        || bytes[5] != 1
        || bytes[18..20] != [62, 0]
    {
        return Err(ERROR.into());
    }
    Ok(())
}
async fn output(path: &Path, cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(path);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|_| "agents.setup.error.probe")?;
    #[cfg(windows)]
    let _job =
        crate::codex::job::ProcessTree::attach(&child).map_err(|_| "agents.setup.error.probe")?;
    let mut stream = child
        .stdout
        .take()
        .ok_or("agents.setup.error.probe")?
        .take(16_385);
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = vec![];
        stream
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "agents.setup.error.probe")?;
        if bytes.len() > 16_384
            || !child
                .wait()
                .await
                .map_err(|_| "agents.setup.error.probe")?
                .success()
        {
            return Err("agents.setup.error.probe".into());
        }
        String::from_utf8(bytes).map_err(|_| "agents.setup.error.probe".into())
    })
    .await
    .map_err(|_| "agents.setup.error.probe");
    let _ = child.kill().await;
    let _ = child.wait().await;
    result?
}
/// Selected files are checked without ever invoking a launcher or reading project configuration.
pub fn verify_executable(path: &Path) -> Result<Candidate, String> {
    if !path.is_absolute() || !path.is_file() {
        return Err("agents.error.executable_invalid".into());
    }
    native_header(path)?;
    let path = dunce::canonicalize(path).map_err(|_| ERROR)?;
    let cwd = tempfile::tempdir().map_err(|_| "agents.setup.error.disk")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "agents.setup.error.probe")?;
    runtime.block_on(async {
        let version = output(&path, cwd.path(), &["--version"]).await?;
        let version = version.trim();
        let value = version
            .strip_prefix("codex-cli ")
            .ok_or("agents.setup.error.probe")?;
        semver::Version::parse(value).map_err(|_| "agents.setup.error.probe")?;
        let help = output(&path, cwd.path(), &["app-server", "--help"]).await?;
        if !help.contains("--listen") {
            return Err("agents.setup.error.probe".into());
        }
        Ok(Candidate {
            path: path.clone(),
            version: version.to_string(),
            source: "Codex CLI".into(),
        })
    })
}
fn copy(source: &mut dyn Read, dest: &mut File, flag: &AtomicBool) -> Result<(), String> {
    let mut buffer = [0; 65536];
    let mut total = 0;
    loop {
        cancelled(flag)?;
        let n = source.read(&mut buffer).map_err(|_| ERROR)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > LIMIT {
            return Err(ERROR.into());
        }
        dest.write_all(&buffer[..n])
            .map_err(|_| "agents.setup.error.disk")?;
    }
    dest.sync_all()
        .map_err(|_| "agents.setup.error.disk".into())
}
fn safe_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.as_os_str().len() <= 4096
        && !path.to_string_lossy().contains(['\\', ':'])
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn entry_destination(
    name: &Path,
    dest: &Path,
    binary: &str,
    seen: &mut std::collections::HashSet<PathBuf>,
) -> Result<PathBuf, String> {
    if !safe_path(name) {
        return Err(ERROR.into());
    }
    let target = if name.file_name().is_some_and(|n| n == binary) {
        dest.to_owned()
    } else {
        dest.parent().ok_or(ERROR)?.join(name)
    };
    let collision = if cfg!(windows) {
        PathBuf::from(target.to_string_lossy().to_lowercase())
    } else {
        target.clone()
    };
    if !seen.insert(collision) {
        return Err(ERROR.into());
    }
    Ok(target)
}
fn extract(
    path: &Path,
    dest: &Path,
    binary: &str,
    zip: bool,
    flag: &AtomicBool,
) -> Result<(), String> {
    const ARCHIVE_LIMIT: u64 = 1024 * 1024 * 1024;
    let mut found = false;
    let mut total = 0;
    let mut seen = std::collections::HashSet::new();
    let mut unpack = |name: &Path,
                      is_dir: bool,
                      size: u64,
                      _executable: bool,
                      source: &mut dyn Read|
     -> Result<(), String> {
        total += size;
        if total > ARCHIVE_LIMIT || size > LIMIT {
            return Err(ERROR.into());
        }
        let target = entry_destination(name, dest, binary, &mut seen)?;
        if is_dir {
            fs::create_dir_all(target).map_err(|_| "agents.setup.error.disk")?;
            return Ok(());
        }
        if name.file_name().is_some_and(|n| n == binary) {
            if found {
                return Err(ERROR.into());
            }
            found = true;
        }
        fs::create_dir_all(target.parent().ok_or(ERROR)?).map_err(|_| "agents.setup.error.disk")?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|_| "agents.setup.error.disk")?;
        copy(source, &mut file, flag)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if target == dest || _executable {
                0o700
            } else {
                0o600
            };
            fs::set_permissions(&target, fs::Permissions::from_mode(mode))
                .map_err(|_| "agents.setup.error.disk")?;
        }
        Ok(())
    };
    if zip {
        let mut archive =
            zip::ZipArchive::new(File::open(path).map_err(|_| ERROR)?).map_err(|_| ERROR)?;
        if archive.len() > 256 {
            return Err(ERROR.into());
        }
        for i in 0..archive.len() {
            cancelled(flag)?;
            let mut entry = archive.by_index(i).map_err(|_| ERROR)?;
            let name = entry.enclosed_name().ok_or(ERROR)?;
            let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
            if !matches!(mode, 0 | 0o100000 | 0o040000) {
                return Err(ERROR.into());
            }
            let is_dir = entry.is_dir();
            let size = entry.size();
            unpack(
                &name,
                is_dir,
                size,
                entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0),
                &mut entry,
            )?;
        }
    } else {
        let gzip = flate2::read::GzDecoder::new(File::open(path).map_err(|_| ERROR)?);
        let mut archive = tar::Archive::new(gzip.take(ARCHIVE_LIMIT + 1024 * 1024));
        for (i, entry) in archive.entries().map_err(|_| ERROR)?.enumerate() {
            cancelled(flag)?;
            if i >= 256 {
                return Err(ERROR.into());
            }
            let mut entry = entry.map_err(|_| ERROR)?;
            let name = entry.path().map_err(|_| ERROR)?.into_owned();
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                return Err(ERROR.into());
            }
            let size = entry.size();
            unpack(
                &name,
                kind.is_dir(),
                size,
                entry.header().mode().map_err(|_| ERROR)? & 0o111 != 0,
                &mut entry,
            )?;
        }
    }
    if !found {
        return Err(ERROR.into());
    }
    Ok(())
}

fn transport_error(error: String) -> String {
    match error.as_str() {
        "updates.status.cancelled" => "agents.setup.cancelled",
        "updates.error.disk" => "agents.setup.error.disk",
        "updates.error.checksum" => "agents.setup.error.checksum",
        "updates.error.size" | "updates.error.url" => "agents.setup.error.package",
        _ => "agents.setup.error.download",
    }
    .into()
}
pub fn install(
    root: &Path,
    supplied: Option<&Path>,
    flag: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<Candidate, String> {
    cancelled(flag)?;
    let package = package()?;
    let target = managed_path(root)?;
    if target.exists() {
        let candidate = verify_executable(&target)?;
        if candidate.version != format!("codex-cli {VERSION}") {
            return Err("agents.setup.error.existing".into());
        }
        return Ok(candidate);
    }
    fs::create_dir_all(root).map_err(|_| "agents.setup.error.disk")?;
    let archive = if let Some(path) = supplied {
        if fs::metadata(path).map_err(|_| ERROR)?.len() != package.size {
            return Err(ERROR.into());
        }
        updates::verify(path, package.hash, flag).map_err(transport_error)?;
        path.to_owned()
    } else {
        updates::acquire_verified(
            &Asset {
                name: package.name.into(),
                size: package.size,
                browser_download_url: format!(
                    "https://github.com/openai/codex/releases/download/rust-v{VERSION}/{}",
                    package.name
                ),
            },
            package.hash,
            &root.join("downloads"),
            flag,
            &progress,
        )
        .map_err(transport_error)?
    };
    cancelled(flag)?;
    progress(Progress {
        bytes: package.size,
        total: package.size,
        verifying: true,
    });
    let stage = tempfile::tempdir_in(root).map_err(|_| "agents.setup.error.disk")?;
    let exe = stage.path().join(executable_name());
    extract(&archive, &exe, package.binary, cfg!(windows), flag)?;
    let candidate = verify_executable(&exe)?;
    if candidate.version != format!("codex-cli {VERSION}") {
        return Err("agents.setup.error.probe".into());
    }
    cancelled(flag)?;
    let directory = target.parent().ok_or(ERROR)?;
    fs::create_dir_all(directory.parent().ok_or(ERROR)?).map_err(|_| "agents.setup.error.disk")?;
    if directory.exists() {
        return Err("agents.setup.error.existing".into());
    }
    fs::write(stage.path().join("TURBODBNOTE-SOURCE.txt"), format!("Official OpenAI Codex CLI {VERSION}\nhttps://github.com/openai/codex/releases/tag/rust-v{VERSION}\nArchive: {}\nSHA256: {}\n",package.name,package.hash)).map_err(|_|"agents.setup.error.disk")?;
    fs::rename(stage.path(), directory).map_err(|_| "agents.setup.error.disk")?;
    Ok(Candidate {
        path: target,
        source: "TurboDBNote".into(),
        version: candidate.version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_paths_and_platform_headers_fail_before_execution() {
        for name in ["../codex", "/codex", "a/../../codex"] {
            assert!(!safe_path(Path::new(name)));
        }
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(executable_name());
        fs::write(&exe, b"not native").unwrap();
        assert!(verify_executable(&exe).is_err());
        assert!(install(dir.path(), None, &AtomicBool::new(true), |_| {}).is_err());
        assert!(!dir.path().join(VERSION).exists());
    }
    #[test]
    fn archives_reject_missing_binary_before_publication() {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.zip");
        let mut zip = ZipWriter::new(File::create(&path).unwrap());
        zip.start_file("other", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.finish().unwrap();
        assert!(
            extract(
                &path,
                &dir.path().join("codex"),
                "codex",
                true,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(!dir.path().join("codex").exists());
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use zip::{ZipWriter, write::SimpleFileOptions};
    #[test]
    fn archive_duplicates_and_traversal_cannot_escape_staging() {
        let dir = tempfile::tempdir().unwrap();
        for names in [vec!["one/codex", "two/codex"], vec!["../codex"]] {
            let archive = dir.path().join("fixture.zip");
            let dest = dir.path().join("stage");
            let mut zip = ZipWriter::new(File::create(&archive).unwrap());
            for name in names {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(b"fixture").unwrap();
            }
            zip.finish().unwrap();
            assert!(extract(&archive, &dest, "codex", true, &AtomicBool::new(false)).is_err());
            assert!(!dir.path().join("codex").exists());
        }
    }
    #[test]
    fn tar_symlink_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("fixture.tar.gz");
        let gzip = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(gzip);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        tar.append_link(&mut header, "codex", "../outside").unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        assert!(
            extract(
                &archive,
                &dir.path().join("stage"),
                "codex",
                false,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(!dir.path().join("stage").exists());
    }
}

#[cfg(test)]
mod package_layout_tests {
    use super::*;
    #[test]
    fn official_bundle_layout_keeps_helpers_resources_and_licenses() {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("official-layout.zip");
        let mut zip = ZipWriter::new(File::create(&archive).unwrap());
        for index in 0..40 {
            zip.start_file(
                format!("resources/license-{index}.txt"),
                SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"fixture").unwrap();
        }
        for name in ["codex-official.exe", "codex-command-runner.exe"] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"fixture").unwrap();
        }
        zip.finish().unwrap();
        let stage = root.path().join("staging");
        let exe = stage.join("codex.exe");
        extract(
            &archive,
            &exe,
            "codex-official.exe",
            true,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(exe.is_file());
        assert!(stage.join("resources/license-39.txt").is_file());
        assert!(stage.join("codex-command-runner.exe").is_file());
    }
}
