//! Versioned driver component packages. Native libraries are only probed in workers.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub const PROTOCOL: u32 = 5;
const LIMIT: u64 = 1024 * 1024 * 1024;
const MANIFEST: &str = "driver-components.json";
const ERROR: &str = "components.error.package";
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub app_version: String,
    pub target: String,
    pub protocol: u32,
    pub files: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
struct Active {
    folder: String,
}
#[derive(Default)]
struct Runtime {
    root: Option<PathBuf>,
    disabled: HashSet<String>,
}
fn runtime() -> &'static RwLock<Runtime> {
    static STATE: OnceLock<RwLock<Runtime>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}
pub fn configure(root: PathBuf, disabled: &[String]) {
    if let Ok(mut state) = runtime().write() {
        state.root = Some(root);
        state.disabled = disabled.iter().cloned().collect();
    }
}
pub fn enabled(id: &str) -> bool {
    runtime().read().is_ok_and(|s| !s.disabled.contains(id))
}
pub fn target() -> &'static str {
    if cfg!(windows) {
        "windows-x64"
    } else {
        "linux-x64"
    }
}
pub fn archive_name(version: &str, target: &str) -> String {
    format!("TurboDBNote-v{version}-{target}-drivers.zip")
}
fn safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && !name.contains(['\\', ':'])
        && name.split('/').all(|s| {
            ![
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9",
            ]
            .contains(
                &s.split('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase()
                    .as_str(),
            ) && !s.is_empty()
                && s != "."
                && s != ".."
                && !s.ends_with(['.', ' '])
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        })
}
fn cancel(flag: &AtomicBool) -> Result<(), String> {
    if flag.load(Ordering::Relaxed) {
        Err("components.cancelled".into())
    } else {
        Ok(())
    }
}
fn disk<T>(_: T) -> String {
    "components.error.disk".into()
}
pub fn hash(path: &Path, cancelled: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(disk)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        cancel(cancelled)?;
        let n = file.read(&mut buffer).map_err(disk)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn no_link(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        let Ok(meta) = fs::symlink_metadata(ancestor) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            return Err(ERROR.into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(ERROR.into());
            }
        }
    }
    Ok(())
}
fn validate(manifest: &Manifest, version: &str, platform: &str) -> Result<(), String> {
    if !semver::Version::parse(version).is_ok_and(|v| v.to_string() == version && v.pre.is_empty())
    {
        return Err("components.error.version".into());
    }
    if manifest.protocol != PROTOCOL
        || manifest.app_version != version
        || manifest.target != platform
    {
        return Err(mismatch(
            &manifest.app_version,
            &manifest.target,
            manifest.protocol,
            version,
            platform,
        ));
    }
    if manifest.schema_version != 1
        || !["windows-x64", "linux-x64"].contains(&platform)
        || manifest.files.is_empty()
        || manifest.files.len() > 128
    {
        return Err(ERROR.into());
    }
    let mut seen = HashSet::new();
    let mut bytes = 0u64;
    for file in &manifest.files {
        if !safe(&file.path)
            || file.path == MANIFEST
            || !seen.insert(file.path.to_ascii_lowercase())
            || file.size == 0
            || file.size > LIMIT / 2
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ERROR.into());
        }
        bytes = bytes.checked_add(file.size).ok_or(ERROR)?;
    }
    let host = if platform == "windows-x64" {
        "turbodbnote-driver-host.exe"
    } else {
        "turbodbnote-driver-host"
    };
    if bytes > LIMIT || !manifest.files.iter().any(|e| e.path == host) {
        return Err(ERROR.into());
    }
    Ok(())
}
fn mismatch(
    version: &str,
    platform: &str,
    protocol: u32,
    expected_version: &str,
    expected_platform: &str,
) -> String {
    // Only fixed-format, non-sensitive metadata may reach the UI.
    let safe = |s: &str| {
        if s.len() <= 64
            && semver::Version::parse(s).is_ok_and(|v| v.pre.is_empty() && v.build.is_empty())
        {
            s.to_owned()
        } else {
            "?".into()
        }
    };
    format!(
        "components.mismatch:{}",
        serde_json::json!({
            "version":safe(version), "platform":if ["windows-x64","linux-x64"].contains(&platform) {platform} else {"?"}, "protocol":protocol,
            "expected_version":safe(expected_version), "expected_platform":expected_platform, "expected_protocol":PROTOCOL
        })
    )
}
pub fn inspect(archive: &Path, version: &str, platform: &str) -> Result<Manifest, String> {
    if fs::metadata(archive).map_err(disk)?.len() > LIMIT {
        return Err(ERROR.into());
    }
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(disk)?).map_err(|_| ERROR)?;
    let file = zip.by_name(MANIFEST).map_err(|_| ERROR)?;
    if file.size() > 65536 {
        return Err(ERROR.into());
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes).map_err(disk)?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    validate(&manifest, version, platform)?;
    Ok(manifest)
}
pub fn install(
    root: &Path,
    archive: &Path,
    version: &str,
    platform: &str,
    flag: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<PathBuf, String> {
    let manifest = inspect(archive, version, platform)?;
    no_link(root)?;
    fs::create_dir_all(root).map_err(disk)?;
    let stage = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root)
        .map_err(disk)?;
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(disk)?).map_err(|_| ERROR)?;
    if zip.len() != manifest.files.len() + 1 {
        return Err(ERROR.into());
    }
    let mut seen = HashSet::new();
    let mut completed = 0u64;
    for n in 0..zip.len() {
        cancel(flag)?;
        let mut file = zip.by_index(n).map_err(|_| ERROR)?;
        if !safe(file.name())
            || !seen.insert(file.name().to_ascii_lowercase())
            || file.is_dir()
            || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err(ERROR.into());
        }
        if file.name() == MANIFEST {
            continue;
        }
        let entry = manifest
            .files
            .iter()
            .find(|e| e.path == file.name())
            .ok_or(ERROR)?;
        if file.size() != entry.size {
            return Err(ERROR.into());
        }
        let path = stage.path().join(&entry.path);
        fs::create_dir_all(path.parent().ok_or(ERROR)?).map_err(disk)?;
        let mut out = File::create(&path).map_err(disk)?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 65536];
        let mut bytes = 0;
        loop {
            cancel(flag)?;
            let n = file.read(&mut buffer).map_err(disk)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > entry.size {
                return Err(ERROR.into());
            }
            out.write_all(&buffer[..n]).map_err(disk)?;
            digest.update(&buffer[..n]);
        }
        if bytes != entry.size
            || format!("{:x}", digest.finalize()) != entry.sha256.to_ascii_lowercase()
        {
            return Err("components.error.checksum".into());
        }
        out.sync_all().map_err(disk)?;
        #[cfg(unix)]
        if entry.path == "turbodbnote-driver-host" {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(disk)?;
        }
        completed += bytes;
        progress(completed);
    }
    fs::write(
        stage.path().join(MANIFEST),
        serde_json::to_vec_pretty(&manifest).map_err(|_| ERROR)?,
    )
    .map_err(disk)?;
    cancel(flag)?;
    let name = format!("{}-{}", version, &hash(archive, flag)?[..16]);
    let destination = root.join(&name);
    if destination.exists() {
        verify_directory(&destination, version, platform, flag)?;
    } else {
        fs::rename(stage.path(), &destination).map_err(disk)?;
    }
    let mut pointer = tempfile::NamedTempFile::new_in(root).map_err(disk)?;
    serde_json::to_writer(pointer.as_file_mut(), &Active { folder: name }).map_err(|_| ERROR)?;
    pointer.as_file().sync_all().map_err(disk)?;
    cancel(flag)?;
    pointer.persist(root.join("active.json")).map_err(disk)?;
    Ok(destination)
}
pub fn verify_directory(
    directory: &Path,
    version: &str,
    platform: &str,
    flag: &AtomicBool,
) -> Result<Manifest, String> {
    no_link(directory)?;
    let mut bytes = Vec::new();
    File::open(directory.join(MANIFEST))
        .map_err(disk)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(disk)?;
    if bytes.len() > 65536 {
        return Err(ERROR.into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    validate(&manifest, version, platform)?;
    for entry in &manifest.files {
        let path = directory.join(&entry.path);
        no_link(&path)?;
        if fs::metadata(&path).map_err(disk)?.len() != entry.size
            || hash(&path, flag)? != entry.sha256.to_ascii_lowercase()
        {
            return Err("components.error.checksum".into());
        }
    }
    Ok(manifest)
}
pub fn resolve(root: &Path, version: &str, platform: &str) -> Result<Option<PathBuf>, String> {
    let pointer = root.join("active.json");
    if !pointer.exists() {
        return Ok(None);
    }
    no_link(root)?;
    let mut bytes = Vec::new();
    File::open(&pointer)
        .map_err(disk)?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(disk)?;
    if bytes.len() > 4096 {
        return Err(ERROR.into());
    }
    let active: Active = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    if !safe(&active.folder) || active.folder.contains('/') {
        return Err(ERROR.into());
    }
    let path = root.join(active.folder);
    let mut data = Vec::new();
    File::open(path.join(MANIFEST))
        .map_err(disk)?
        .take(65537)
        .read_to_end(&mut data)
        .map_err(disk)?;
    if data.len() > 65536 {
        return Err(ERROR.into());
    }
    let stored: Manifest = serde_json::from_slice(&data).map_err(|_| ERROR)?;
    if stored.schema_version == 1
        && stored.protocol <= PROTOCOL
        && stored.target == platform
        && stored.app_version != version
    {
        return Ok(None);
    }
    verify_directory(&path, version, platform, &AtomicBool::new(false))?;
    Ok(Some(path))
}
pub fn runtime_directory() -> Result<Option<PathBuf>, String> {
    let root = runtime().read().map_err(|_| ERROR)?.root.clone();
    if let Some(root) = root
        && let Some(path) = resolve(&root, env!("CARGO_PKG_VERSION"), target())?
    {
        return Ok(Some(path));
    }
    let bundled = crate::external_driver::bundled_directory()
        .filter(|p| p.join(crate::external_driver::executable_name()).is_file());
    if let Some(path) = &bundled
        && path.join(MANIFEST).is_file()
    {
        verify_directory(
            path,
            env!("CARGO_PKG_VERSION"),
            target(),
            &AtomicBool::new(false),
        )?;
    }
    Ok(bundled)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    pub protocol: u32,
    pub app_version: String,
    pub target: String,
    pub driver_id: String,
    pub ready: bool,
    pub reason: String,
}
pub fn probe(directory: &Path, driver: &str, flag: &AtomicBool) -> Result<Probe, String> {
    if !crate::driver_catalog::DRIVERS
        .iter()
        .any(|d| d.id == driver && d.kind == crate::DatabaseKind::External)
    {
        return Err(ERROR.into());
    }
    let mut command =
        std::process::Command::new(directory.join(crate::external_driver::executable_name()));
    command
        .args(["--probe", driver])
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|_| "components.error.probe")?;
    let stdout = child.stdout.take().ok_or(ERROR)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(65537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = std::time::Instant::now();
    let status = loop {
        if flag.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if flag.load(Ordering::Relaxed) {
                "components.cancelled"
            } else {
                "components.error.probe"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("components.error.probe".into());
            }
        }
    };
    let bytes = reader.join().map_err(|_| ERROR)?.map_err(disk)?;
    if !status.success() || bytes.len() > 65536 {
        return Err("components.error.probe".into());
    }
    let probe: Probe = serde_json::from_slice(&bytes).map_err(|_| "components.error.probe")?;
    if probe.driver_id != driver {
        return Err("components.error.probe".into());
    }
    if probe.protocol != PROTOCOL
        || probe.app_version != env!("CARGO_PKG_VERSION")
        || probe.target != target()
    {
        return Err(mismatch(
            &probe.app_version,
            &probe.target,
            probe.protocol,
            env!("CARGO_PKG_VERSION"),
            target(),
        ));
    }
    Ok(probe)
}
pub fn build(directory: &Path, output: &Path, version: &str, platform: &str) -> Result<(), String> {
    fn walk(root: &Path, dir: &Path, files: &mut Vec<Entry>) -> Result<(), String> {
        no_link(dir)?;
        for file in fs::read_dir(dir).map_err(disk)? {
            let file = file.map_err(disk)?;
            no_link(&file.path())?;
            if file.file_type().map_err(disk)?.is_dir() {
                walk(root, &file.path(), files)?;
            } else {
                let name = file
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| ERROR)?
                    .to_string_lossy()
                    .replace('\\', "/");
                if name == MANIFEST || name.ends_with(".pdb") {
                    continue;
                }
                if !safe(&name) {
                    return Err(ERROR.into());
                }
                files.push(Entry {
                    path: name,
                    size: file.metadata().map_err(disk)?.len(),
                    sha256: hash(&file.path(), &AtomicBool::new(false))?,
                });
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(directory, directory, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest = Manifest {
        schema_version: 1,
        app_version: version.into(),
        target: platform.into(),
        protocol: PROTOCOL,
        files,
    };
    validate(&manifest, version, platform)?;
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .map_err(disk)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in &manifest.files {
        zip.start_file(&entry.path, options).map_err(|_| ERROR)?;
        std::io::copy(
            &mut File::open(directory.join(&entry.path)).map_err(disk)?,
            &mut zip,
        )
        .map_err(disk)?;
    }
    zip.start_file(MANIFEST, options).map_err(|_| ERROR)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|_| ERROR)?)
        .map_err(disk)?;
    zip.finish().map_err(|_| ERROR)?;
    fs::write(
        directory.join(MANIFEST),
        serde_json::to_vec_pretty(&manifest).map_err(|_| ERROR)?,
    )
    .map_err(disk)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn mismatch_reports_only_valid_version_and_platform_metadata() {
        let error = super::mismatch("0.1.1", "windows-x64", 1, "0.1.1", "windows-x64");
        let value: serde_json::Value =
            serde_json::from_str(error.strip_prefix("components.mismatch:").unwrap()).unwrap();
        assert_eq!(value["protocol"], 1);
        assert_eq!(value["expected_protocol"], super::PROTOCOL);
        let error = super::mismatch("invalid", "invalid", 0, "0.1.1", "windows-x64");
        let value: serde_json::Value =
            serde_json::from_str(error.strip_prefix("components.mismatch:").unwrap()).unwrap();
        assert_eq!(value["version"], "?");
        assert_eq!(value["platform"], "?");
    }
    use super::*;
    #[test]
    fn packages_are_versioned_verified_and_do_not_overwrite_tampered_files() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        fs::create_dir(&native).unwrap();
        fs::write(
            native.join(crate::external_driver::executable_name()),
            b"fixture host bytes",
        )
        .unwrap();
        let archive = dir.path().join("drivers.zip");
        build(&native, &archive, "0.1.1", target()).unwrap();
        let root = dir.path().join("installed");
        let active = install(
            &root,
            &archive,
            "0.1.1",
            target(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            resolve(&root, "0.1.1", target()).unwrap(),
            Some(active.clone())
        );
        assert!(inspect(&archive, "0.1.2", target()).is_err());
        fs::write(
            active.join(crate::external_driver::executable_name()),
            b"tampered",
        )
        .unwrap();
        assert!(resolve(&root, "0.1.1", target()).is_err());
        assert!(
            install(
                &root,
                &archive,
                "0.1.1",
                target(),
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
    }
    #[test]
    fn manifest_checks_platform_protocol_and_archive_members() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        fs::create_dir(&native).unwrap();
        fs::write(
            native.join(crate::external_driver::executable_name()),
            b"test",
        )
        .unwrap();
        let archive = dir.path().join("test.zip");
        build(&native, &archive, "0.1.1", target()).unwrap();
        let mut manifest = inspect(&archive, "0.1.1", target()).unwrap();
        manifest.protocol += 1;
        assert!(validate(&manifest, "0.1.1", target()).is_err());
        assert!(
            inspect(
                &archive,
                "0.1.1",
                if target() == "windows-x64" {
                    "linux-x64"
                } else {
                    "windows-x64"
                }
            )
            .is_err()
        );
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&archive)
            .unwrap();
        let mut zip = zip::ZipWriter::new_append(file).unwrap();
        zip.start_file("../unexpected", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"unsafe").unwrap();
        zip.finish().unwrap();
        assert!(
            install(
                &dir.path().join("cache"),
                &archive,
                "0.1.1",
                target(),
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
    }
    #[test]
    fn unsafe_archive_paths_and_cancelled_installations_are_rejected() {
        for path in [
            "../host.exe",
            "/host.exe",
            "C:/host.exe",
            "oracle/../oci.dll",
            "oracle\\oci.dll",
            "host.exe ",
            "a//b",
        ] {
            assert!(!safe(path));
        }
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        fs::create_dir(&native).unwrap();
        fs::write(
            native.join(crate::external_driver::executable_name()),
            b"test",
        )
        .unwrap();
        let archive = dir.path().join("test.zip");
        build(&native, &archive, "0.1.1", target()).unwrap();
        assert!(
            install(
                &dir.path().join("cache"),
                &archive,
                "0.1.1",
                target(),
                &AtomicBool::new(true),
                |_| {}
            )
            .is_err()
        );
    }
}
