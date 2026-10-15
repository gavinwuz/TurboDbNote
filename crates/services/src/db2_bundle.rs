//! Stage the unchanged IBM Windows client archive for explicit, offline installation.
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
pub const VERSION: &str = "12.1.4";
pub const URL: &str = "https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/v12.1.4/ntx64_odbc_cli.zip";
pub const SHA256: &str = "28748e451e628c41a49676548439c980f02155890d4126342c167fed27a0d63f";
pub const ARCHIVE: &str = "ibm-clidriver-12.1.4-windows-x64.zip";
pub fn prepare(
    cache: &Path,
    native: &Path,
    supplied: Option<&Path>,
    flag: &AtomicBool,
) -> Result<(), String> {
    let error = || "components.error.package".to_string();
    fs::create_dir_all(cache).map_err(|_| error())?;
    let cached = cache.join(ARCHIVE);
    let archive = supplied.unwrap_or(&cached);
    if supplied.is_none() && !cached.exists() {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| error())?;
        let mut response = client
            .get(URL)
            .send()
            .map_err(|_| "updates.error.request")?
            .error_for_status()
            .map_err(|_| "updates.error.request")?;
        let mut temp = tempfile::NamedTempFile::new_in(cache).map_err(|_| error())?;
        let mut buffer = [0; 65536];
        let mut total = 0;
        loop {
            if flag.load(Ordering::Relaxed) {
                return Err("components.cancelled".into());
            }
            let count = response.read(&mut buffer).map_err(|_| error())?;
            if count == 0 {
                break;
            }
            total += count;
            if total > 256 * 1024 * 1024 {
                return Err(error());
            }
            temp.write_all(&buffer[..count]).map_err(|_| error())?;
        }
        if crate::driver_packages::hash(temp.path(), flag)? != SHA256 {
            return Err(error());
        }
        temp.persist_noclobber(&cached).map_err(|_| error())?;
    }
    if crate::driver_packages::hash(archive, flag)? != SHA256 {
        return Err(error());
    }
    let destination = native.join("db2-offline");
    fs::create_dir_all(&destination).map_err(|_| error())?;
    fs::copy(archive, destination.join(ARCHIVE)).map_err(|_| error())?;
    fs::write(
        destination.join("SHA256SUMS"),
        format!("{SHA256}  {ARCHIVE}\n"),
    )
    .map_err(|_| error())?;
    fs::write(destination.join("README.txt"), format!("IBM Data Server Driver for ODBC and CLI {VERSION}, Windows x64\nOriginal archive: {URL}\nSHA256: {SHA256}\n\nRead the license materials inside the unchanged IBM archive. Extract it into a permanent directory. To register it, open an Administrator Command Prompt in clidriver\\bin and explicitly run:\n  db2cli.exe install -setup\nThen refresh Settings > Database > Drivers. No registration is performed by TurboDBNote. Do not move the directory after registration. An installed driver does not verify server connectivity. This adapter targets Db2 LUW; IBM i / z/OS may require separate Db2 Connect licensing.\n\nInstallation reference: https://www.ibm.com/support/pages/db2-odbc-cli-driver-download-and-installation-information\n\n中文：先阅读原始 ZIP 内的许可，解压至永久目录；在管理员命令提示符进入 clidriver\\bin，手动执行 db2cli.exe install -setup，再刷新驱动检测。应用不会自动注册或修改系统驱动。\n")).map_err(|_| error())?;
    Ok(())
}
