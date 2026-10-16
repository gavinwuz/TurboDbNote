//! Stage the unchanged IBM Windows client archive for explicit, offline installation.
use std::{fs, path::Path, sync::atomic::AtomicBool};
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
    let archive = crate::bundle_download::Archive {
        component: "IBM Db2 CLI 12.1.4 Windows x64",
        url: URL,
        name: ARCHIVE,
        sha256: SHA256,
        size: None,
        offline: "-Db2ClientArchive",
    }
    .acquire(cache, supplied, flag)?;
    let destination = native.join("db2-offline");
    fs::create_dir_all(&destination).map_err(|_| error())?;
    fs::copy(&archive, destination.join(ARCHIVE)).map_err(|_| error())?;
    fs::write(
        destination.join("SHA256SUMS"),
        format!("{SHA256}  {ARCHIVE}\n"),
    )
    .map_err(|_| error())?;
    fs::write(destination.join("README.txt"), format!("IBM Data Server Driver for ODBC and CLI {VERSION}, Windows x64\nOriginal archive: {URL}\nSHA256: {SHA256}\n\nRead the license materials inside the unchanged IBM archive. Extract it into a permanent directory. To register it, open an Administrator Command Prompt in clidriver\\bin and explicitly run:\n  db2cli.exe install -setup\nThen refresh Settings > Database > Drivers. No registration is performed by TurboDBNote. Do not move the directory after registration. An installed driver does not verify server connectivity. This adapter targets Db2 LUW; IBM i / z/OS may require separate Db2 Connect licensing.\n\nInstallation reference: https://www.ibm.com/support/pages/db2-odbc-cli-driver-download-and-installation-information\n\n中文：先阅读原始 ZIP 内的许可，解压至永久目录；在管理员命令提示符进入 clidriver\\bin，手动执行 db2cli.exe install -setup，再刷新驱动检测。应用不会自动注册或修改系统驱动。\n")).map_err(|_| error())?;
    Ok(())
}
