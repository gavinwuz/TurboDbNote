//! Build-machine acquisition of Oracle's pinned Basic client; no driver is loaded here.
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
pub const VERSION: &str = "21.23.0.0.0";
fn error<T>(_: T) -> String {
    "components.error.package".into()
}
pub fn prepare(
    cache: &Path,
    native: &Path,
    platform: &str,
    supplied: Option<&Path>,
    flag: &AtomicBool,
) -> Result<(), String> {
    let (url, size, sha) = match platform {
        "windows-x64" => (
            "https://download.oracle.com/otn_software/nt/instantclient/2123000/instantclient-basic-windows.x64-21.23.0.0.0dbru.zip",
            89742997,
            "f01253058bf786678326b5a7aee8131bf559f210bba144f425fd8114eefcf0b9",
        ),
        "linux-x64" => (
            "https://download.oracle.com/otn_software/linux/instantclient/2123000/instantclient-basic-linux.x64-21.23.0.0.0dbru.zip",
            83595164,
            "68a6ccd7ca6fbfb4d2914bd6531a8599fdb75841b8d47df5256bfef40d020820",
        ),
        _ => return Err("components.error.version".into()),
    };
    let archive = crate::bundle_download::Archive {
        component: if platform == "windows-x64" {
            "Oracle 21.23 Windows x64"
        } else {
            "Oracle 21.23 Linux x64"
        },
        url,
        name: if platform == "windows-x64" {
            "oracle-21.23.0.0.0-windows-x64.zip"
        } else {
            "oracle-21.23.0.0.0-linux-x64.zip"
        },
        sha256: sha,
        size: Some(size),
        offline: "-OracleClientArchive",
    }
    .acquire(cache, supplied, flag)?;
    let destination = native.join("oracle");
    if destination.exists() {
        return Err("components.error.package".into());
    }
    let stage = tempfile::tempdir_in(native).map_err(error)?;
    let mut zip = zip::ZipArchive::new(File::open(&archive).map_err(error)?).map_err(error)?;
    let mut names = std::collections::HashSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        if flag.load(Ordering::Relaxed) {
            return Err("components.cancelled".into());
        }
        let mut file = zip.by_index(i).map_err(error)?;
        let name = file.name().to_string();
        if name.starts_with("META-INF/") {
            continue;
        }
        let mut parts = name.split('/');
        if parts.next() != Some("instantclient_21_23") {
            return Err("components.error.package".into());
        }
        let Some(base) = parts.next().filter(|s| !s.is_empty()) else {
            continue;
        };
        if parts.next().is_some() {
            continue;
        }
        let base = base.to_string();
        let lower = base.to_ascii_lowercase();
        let library = if platform == "windows-x64" {
            lower.ends_with(".dll")
        } else {
            lower.contains(".so")
        };
        if !(library || lower.contains("license") || lower == "basic_readme")
            || lower.contains("jdbc")
        {
            continue;
        }
        if !base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            || !names.insert(base.to_ascii_lowercase())
        {
            return Err("components.error.package".into());
        }
        if file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            let mut target = String::new();
            file.by_ref()
                .take(257)
                .read_to_string(&mut target)
                .map_err(error)?;
            if target.len() > 256 || target.contains(['/', '\\']) || target == ".." {
                return Err("components.error.package".into());
            }
            drop(file);
            file = zip
                .by_name(&format!("instantclient_21_23/{target}"))
                .map_err(error)?;
            if file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("components.error.package".into());
            }
        }
        let mut out = File::create(stage.path().join(&base)).map_err(error)?;
        let copied = std::io::copy(&mut file.by_ref().take(512 * 1024 * 1024 + 1), &mut out)
            .map_err(error)?;
        total += copied;
        if copied != file.size() || total > 1024 * 1024 * 1024 {
            return Err("components.error.package".into());
        }
    }
    if !stage
        .path()
        .join(if platform == "windows-x64" {
            "oci.dll"
        } else {
            "libclntsh.so"
        })
        .is_file()
        || !names.iter().any(|s| s.contains("license"))
    {
        return Err("components.error.package".into());
    }
    fs::write(stage.path().join("PROVENANCE.json"),serde_json::to_vec_pretty(&serde_json::json!({"version":VERSION,"url":url,"sha256":sha,"platform":platform,"reference":"https://www.oracle.com/database/technologies/instant-client/downloads.html"})).map_err(error)?).map_err(error)?;
    fs::rename(stage.path(), destination).map_err(error)?;
    Ok(())
}
