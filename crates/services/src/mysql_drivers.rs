//! Version-pinned driver acquisition. Never loads native code in the GUI.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
pub const ID: &str = "mysql-5.1.73-winx64";
pub const URL: &str = "https://cdn.mysql.com/archives/mysql-5.1/mysql-noinstall-5.1.73-winx64.zip";
pub const ARCHIVE_SHA: &str = "f7e91fcd0dddb62af8800c5f0067a92d437155384b878db9eb1e846869631cda";
pub const DLL_SHA: &str = "512c5b537a46d162a102ec2e99f36558f057f2b306ff52ef45999016a927acdd";
pub const SOURCE_URL: &str = "https://cdn.mysql.com/archives/mysql-5.1/mysql-5.1.73.tar.gz";
pub const SOURCE_SHA: &str = "05ebe21305408b24407d14b77607a3e5ffa3c300e03f1359d3066f301989dcb5";
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("已取消驱动下载 / 导入".into())
    } else {
        Ok(())
    }
}
pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(err)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn validate_pinned(path: &Path) -> Result<(), String> {
    if fs::metadata(path).map_err(err)?.len() != 2664448 || hash_file(path)? != DLL_SHA {
        return Err(
            "驱动不是已验证的 MySQL 5.1.73 x64 发行库；请导入官方 ZIP 或使用连接的自定义 DLL 路径"
                .into(),
        );
    }
    Ok(())
}
pub fn installed(root: &Path) -> PathBuf {
    root.join(ID).join("libmysql.dll")
}
pub fn bundled() -> Option<PathBuf> {
    let path = installed(&std::env::current_exe().ok()?.parent()?.join("drivers"));
    path.is_file().then_some(path)
}
fn download(
    url: &str,
    expected: &str,
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(String),
) -> Result<(), String> {
    cancelled(cancel)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(err)?;
    runtime.block_on(async {
        let work = async {
            let client = reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(600))
                .build()
                .map_err(err)?;
            let mut response = client
                .get(url)
                .send()
                .await
                .map_err(err)?
                .error_for_status()
                .map_err(err)?;
            let total = response.content_length().unwrap_or(0);
            let mut output = File::create(path).map_err(err)?;
            let mut done = 0u64;
            let mut last = 0;
            let mut hash = Sha256::new();
            while let Some(bytes) = response.chunk().await.map_err(err)? {
                cancelled(cancel)?;
                done += bytes.len() as u64;
                if done > 150 * 1024 * 1024 {
                    return Err("下载文件超过限制".into());
                }
                output.write_all(&bytes).map_err(err)?;
                hash.update(&bytes);
                if done - last >= 1024 * 1024 {
                    progress(format!("下载 {} / {} MiB", done / 1048576, total / 1048576));
                    last = done;
                }
            }
            if format!("{:x}", hash.finalize()) != expected {
                return Err("下载校验失败，未安装；请重试".into());
            }
            output.sync_all().map_err(err)?;
            Ok(())
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                result=&mut work=>break result,
                _=tokio::time::sleep(Duration::from_millis(100))=>cancelled(cancel)?,
            }
        }
    })
}
/// Imports only exact known archive entries, not archive-controlled filesystem paths.
pub fn install(
    root: &Path,
    archive: Option<&Path>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(String),
) -> Result<PathBuf, String> {
    fs::create_dir_all(root).map_err(err)?;
    let stage = tempfile::Builder::new()
        .prefix(".mysql-install-")
        .tempdir_in(root)
        .map_err(err)?;
    let downloaded = stage.path().join("download.zip");
    let archive = match archive {
        Some(p) => p,
        None => {
            download(URL, ARCHIVE_SHA, &downloaded, cancel, &mut progress)?;
            &downloaded
        }
    };
    cancelled(cancel)?;
    progress("校验官方压缩包…".into());
    if fs::metadata(archive).map_err(err)?.len() != 124989691 || hash_file(archive)? != ARCHIVE_SHA
    {
        return Err("ZIP 校验失败：仅接受官方 mysql-noinstall-5.1.73-winx64.zip".into());
    }
    let payload = stage.path().join(ID);
    fs::create_dir(&payload).map_err(err)?;
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(err)?).map_err(err)?;
    for (entry, target) in [
        ("bin/libmysql.dll", "libmysql.dll"),
        ("COPYING", "COPYING"),
        ("README", "README.mysql"),
    ] {
        cancelled(cancel)?;
        let mut file = zip.by_name(&format!("{ID}/{entry}")).map_err(err)?;
        if file.size() > 16 * 1024 * 1024 {
            return Err("驱动归档条目过大".into());
        }
        let mut output = File::create(payload.join(target)).map_err(err)?;
        std::io::copy(&mut file, &mut output).map_err(err)?;
        output.sync_all().map_err(err)?;
    }
    validate_pinned(&payload.join("libmysql.dll"))?;
    fs::write(payload.join("provenance.json"),serde_json::to_vec_pretty(&serde_json::json!({"id":ID,"url":URL,"archive_sha256":ARCHIVE_SHA,"dll_sha256":DLL_SHA,"source_url":SOURCE_URL})).map_err(err)?).map_err(err)?;
    cancelled(cancel)?;
    let target = root.join(ID);
    if target.exists() {
        if validate_pinned(&installed(root)).is_ok() {
            return Ok(installed(root));
        }
        // Preserve a damaged installation until the replacement can be committed.
        let backup = stage.path().join("previous");
        fs::rename(&target, &backup)
            .map_err(|_| "旧驱动正在使用或无法替换；请断开兼容连接后重试".to_string())?;
        if let Err(error) = fs::rename(&payload, &target) {
            if let Err(rollback) = fs::rename(&backup, &target) {
                let preserved = stage.keep();
                return Err(format!(
                    "安装失败 {error}；回滚失败 {rollback}；旧目录保留在 {}",
                    preserved.display()
                ));
            }
            return Err(err(error));
        }
    } else {
        fs::rename(&payload, &target).map_err(err)?;
    }
    Ok(installed(root))
}
/// Source accompanies binary redistribution in release packages.
pub fn prepare_source(root: &Path, local: Option<&Path>) -> Result<(), String> {
    let target = root.join(ID).join("mysql-5.1.73-source.tar.gz");
    if let Some(source) = local {
        if hash_file(source)? != SOURCE_SHA {
            return Err("MySQL 源码包校验失败".into());
        }
        fs::copy(source, target).map_err(err)?;
    } else {
        download(
            SOURCE_URL,
            SOURCE_SHA,
            &target,
            &AtomicBool::new(false),
            &mut |_| {},
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_untrusted_archive_without_installing() {
        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("fake.zip");
        fs::write(&fake, b"untrusted").unwrap();
        assert!(install(root.path(), Some(&fake), &AtomicBool::new(false), |_| {}).is_err());
        assert!(!installed(root.path()).exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
    #[test]
    fn cancellation_does_not_leave_driver_or_staging() {
        let root = tempfile::tempdir().unwrap();
        assert!(install(root.path(), None, &AtomicBool::new(true), |_| {}).is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[test]
    fn rejects_wrong_dll() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(validate_pinned(f.path()).is_err());
    }
}
