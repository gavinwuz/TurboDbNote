//! Stable release checks and bounded, cancellable downloads. No credentials or raw HTTP errors.
use reqwest::{Url, blocking::Client};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const RELEASES: &str = "https://github.com/gavinwuz/TurboDbNote/releases";
const API: &str = "https://api.github.com/repos/gavinwuz/TurboDbNote/releases/latest";
const MAX_PACKAGE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_METADATA: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    WindowsInstaller,
    WindowsPortable,
    Linux,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}
#[derive(Debug)]
pub enum Check {
    NoRelease,
    Current,
    Available(Release),
}
#[derive(Clone, Debug)]
pub struct Download {
    pub path: PathBuf,
    pub sha256: String,
    pub kind: PackageKind,
    pub bytes: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub bytes: u64,
    pub total: u64,
    pub verifying: bool,
}

pub fn trusted_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && matches!(
                url.host_str(),
                Some(
                    "api.github.com"
                        | "github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            )
    })
}
fn client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(concat!("TurboDBNote/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !trusted_url(attempt.url().as_str()) {
                attempt.error("update redirect rejected")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "updates.error.request".into())
}
fn bounded(mut response: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "updates.error.request")?;
    if bytes.len() as u64 > limit {
        return Err("updates.error.size".into());
    }
    Ok(bytes)
}
pub fn latest(current: &str) -> Result<Check, String> {
    let client = client(Duration::from_secs(15))?;
    let response = client
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| "updates.error.request")?;
    if response.status().as_u16() == 404 {
        return Ok(Check::NoRelease);
    }
    if matches!(response.status().as_u16(), 403 | 429) {
        return Err("updates.error.rate_limit".into());
    }
    if !response.status().is_success() {
        return Err("updates.error.request".into());
    }
    parse_release(&bounded(response, MAX_METADATA)?, current)
}
fn parse_release(bytes: &[u8], current: &str) -> Result<Check, String> {
    let release: Release =
        serde_json::from_slice(bytes).map_err(|_| "updates.error.version_invalid")?;
    let version = semver::Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|_| "updates.error.version_invalid")?;
    let current = semver::Version::parse(current).map_err(|_| "updates.error.app_version")?;
    if release.draft || release.prerelease || !version.pre.is_empty() {
        return Ok(Check::NoRelease);
    }
    let expected = format!("{RELEASES}/tag/{}", release.tag_name);
    if release.html_url != expected || !trusted_url(&release.html_url) {
        return Err("updates.error.url".into());
    }
    Ok(if version > current {
        Check::Available(release)
    } else {
        Check::Current
    })
}
impl Release {
    pub fn package(&self, kind: PackageKind) -> Result<&Asset, String> {
        let version =
            semver::Version::parse(self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name))
                .map_err(|_| "updates.error.version_invalid")?;
        let suffix = match kind {
            PackageKind::WindowsInstaller => "windows-x64-setup.exe",
            PackageKind::WindowsPortable => "windows-x64-portable.zip",
            PackageKind::Linux => "linux-x64.tar.gz",
        };
        let minimal = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("package-flavor.txt")))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|s| s.trim() == "minimal");
        let suffix = if minimal {
            match kind {
                PackageKind::WindowsInstaller => "windows-x64-minimal-setup.exe",
                PackageKind::WindowsPortable => "windows-x64-minimal-portable.zip",
                PackageKind::Linux => "linux-x64-minimal.tar.gz",
            }
        } else {
            suffix
        };
        self.asset(&format!("TurboDBNote-v{version}-{suffix}"))
    }
    fn asset(&self, name: &str) -> Result<&Asset, String> {
        let mut assets = self.assets.iter().filter(|asset| asset.name == name);
        let asset = assets.next().ok_or("updates.error.package_missing")?;
        if assets.next().is_some() || asset.size == 0 || asset.size > MAX_PACKAGE {
            return Err("updates.error.size".into());
        }
        let expected = format!("{RELEASES}/download/{}/{}", self.tag_name, asset.name);
        if asset.browser_download_url != expected || !trusted_url(&asset.browser_download_url) {
            return Err("updates.error.url".into());
        }
        Ok(asset)
    }
}
fn checksum(text: &str, name: &str) -> Result<String, String> {
    let mut found = None;
    for line in text.lines() {
        if line.len() < 66 || !line.is_char_boundary(64) {
            continue;
        }
        let (hash, rest) = line.split_at(64);
        if rest.strip_prefix("  ").or_else(|| rest.strip_prefix(" *")) == Some(name) {
            if found.is_some() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("updates.error.checksum".into());
            }
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| "updates.error.checksum".into())
}
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("updates.status.cancelled".into())
    } else {
        Ok(())
    }
}
pub fn download(
    release: &Release,
    kind: PackageKind,
    directory: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<Download, String> {
    cancelled(cancel)?;
    let package = release.package(kind)?;
    let sums = release.asset("SHA256SUMS.txt")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "updates.error.request")?;
    runtime.block_on(async {
        tokio::select! {
            result = download_transfer(package, &sums.browser_download_url, kind, directory, cancel, &progress) => result,
            _ = wait_for_cancel(cancel) => Err("updates.status.cancelled".into()),
        }
    })
}
/// Acquire the exact application version's driver asset using the same trusted release transport.
pub fn download_components(
    version: &str,
    directory: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<PathBuf, String> {
    let version = semver::Version::parse(version).map_err(|_| "updates.error.app_version")?;
    let response = client(Duration::from_secs(15))?
        .get(format!(
            "https://api.github.com/repos/gavinwuz/TurboDbNote/releases/tags/v{version}"
        ))
        .send()
        .map_err(|_| "updates.error.request")?;
    if response.status().as_u16() == 404 {
        return Err("components.error.unpublished".into());
    }
    if !response.status().is_success() {
        return Err("updates.error.request".into());
    }
    let release = match parse_release(&bounded(response, MAX_METADATA)?, "0.0.0")? {
        Check::Available(release) => release,
        _ => return Err("components.error.unpublished".into()),
    };
    if release.tag_name != format!("v{version}") {
        return Err("updates.error.version_invalid".into());
    }
    let asset = release
        .asset(&crate::driver_packages::archive_name(
            &version.to_string(),
            crate::driver_packages::target(),
        ))
        .map_err(|e| {
            if e == "updates.error.package_missing" {
                "components.error.unpublished".into()
            } else {
                e
            }
        })?;
    let sums = release.asset("SHA256SUMS.txt")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "updates.error.request")?;
    let kind = if cfg!(windows) {
        PackageKind::WindowsPortable
    } else {
        PackageKind::Linux
    };
    runtime.block_on(async { tokio::select! { result = download_transfer(asset, &sums.browser_download_url, kind, directory, cancel, &progress) => result.map(|p| p.path), _ = wait_for_cancel(cancel) => Err("updates.status.cancelled".into()) } })
}
async fn wait_for_cancel(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn fetch(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "updates.error.request")?;
    match response.status().as_u16() {
        403 | 429 => Err("updates.error.rate_limit".into()),
        200..=299 => Ok(response),
        _ => Err("updates.error.request".into()),
    }
}
async fn download_transfer(
    package: &Asset,
    sums_url: &str,
    kind: PackageKind,
    directory: &Path,
    cancel: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<Download, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        .user_agent(concat!("TurboDBNote/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !trusted_url(attempt.url().as_str()) {
                attempt.error("update redirect rejected")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "updates.error.request")?;
    let mut sums = fetch(&client, sums_url).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = sums.chunk().await.map_err(|_| "updates.error.request")? {
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err("updates.error.size".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let hash = checksum(
        std::str::from_utf8(&bytes).map_err(|_| "updates.error.checksum")?,
        &package.name,
    )?;
    cancelled(cancel)?;
    std::fs::create_dir_all(directory).map_err(|_| "updates.error.disk")?;
    let path = directory.join(&package.name);
    if path.exists() {
        verify(&path, &hash, cancel)?;
        return Ok(Download {
            path,
            sha256: hash,
            kind,
            bytes: package.size,
        });
    }
    let mut source = fetch(&client, &package.browser_download_url).await?;
    let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|_| "updates.error.disk")?;
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut last = Instant::now();
    progress(Progress {
        bytes,
        total: package.size,
        verifying: false,
    });
    while let Some(chunk) = source.chunk().await.map_err(|_| "updates.error.request")? {
        cancelled(cancel)?;
        bytes += chunk.len() as u64;
        if bytes > package.size || bytes > MAX_PACKAGE {
            return Err("updates.error.size".into());
        }
        temp.write_all(&chunk).map_err(|_| "updates.error.disk")?;
        digest.update(&chunk);
        if last.elapsed() >= Duration::from_millis(250) {
            progress(Progress {
                bytes,
                total: package.size,
                verifying: false,
            });
            last = Instant::now();
        }
    }
    progress(Progress {
        bytes,
        total: package.size,
        verifying: true,
    });
    cancelled(cancel)?;
    if bytes != package.size {
        return Err("updates.error.size".into());
    }
    if format!("{:x}", digest.finalize()) != hash {
        return Err("updates.error.checksum".into());
    }
    temp.as_file()
        .sync_all()
        .map_err(|_| "updates.error.disk")?;
    temp.persist_noclobber(&path)
        .map_err(|_| "updates.error.disk")?;
    Ok(Download {
        path,
        sha256: hash,
        kind,
        bytes: package.size,
    })
}

fn stream(
    source: &mut impl Read,
    destination: &mut impl Write,
    size: u64,
    expected: &str,
    cancel: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<(), String> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0; 64 * 1024];
    let mut last = Instant::now();
    progress(Progress {
        bytes,
        total: size,
        verifying: false,
    });
    loop {
        cancelled(cancel)?;
        let read = source
            .read(&mut buffer)
            .map_err(|_| "updates.error.request")?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > size || bytes > MAX_PACKAGE {
            return Err("updates.error.size".into());
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|_| "updates.error.disk")?;
        hash.update(&buffer[..read]);
        if last.elapsed() >= Duration::from_millis(250) {
            progress(Progress {
                bytes,
                total: size,
                verifying: false,
            });
            last = Instant::now();
        }
    }
    progress(Progress {
        bytes,
        total: size,
        verifying: true,
    });
    cancelled(cancel)?;
    if bytes != size {
        return Err("updates.error.size".into());
    }
    if format!("{:x}", hash.finalize()) != expected {
        return Err("updates.error.checksum".into());
    }
    Ok(())
}
pub fn verify(path: &Path, expected: &str, cancel: &AtomicBool) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| "updates.error.disk")?;
    let size = file.metadata().map_err(|_| "updates.error.disk")?.len();
    if size == 0 || size > MAX_PACKAGE {
        return Err("updates.error.size".into());
    }
    stream(
        &mut file,
        &mut std::io::sink(),
        size,
        expected,
        cancel,
        &|_| {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"tag_name": tag, "html_url": format!("{RELEASES}/tag/{tag}"), "assets": []})).unwrap()
    }
    #[test]
    fn stable_version_comparison_and_untrusted_release() {
        assert!(matches!(
            parse_release(&release("v0.2.0"), "0.1.1").unwrap(),
            Check::Available(_)
        ));
        for tag in ["v0.1.1", "v0.1.0"] {
            assert!(matches!(
                parse_release(&release(tag), "0.1.1").unwrap(),
                Check::Current
            ));
        }
        assert!(matches!(
            parse_release(&release("v0.2.0-beta.1"), "0.1.1").unwrap(),
            Check::NoRelease
        ));
        assert!(parse_release(&release("invalid"), "0.1.1").is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&release("v0.2.0")).unwrap();
        value["html_url"] = "https://example.com/update".into();
        assert!(parse_release(&serde_json::to_vec(&value).unwrap(), "0.1.1").is_err());
    }
    #[test]
    fn urls_require_https_and_exact_hosts() {
        assert!(trusted_url(
            "https://release-assets.githubusercontent.com/file?signature=opaque"
        ));
        for url in [
            "http://github.com/file",
            "https://github.com.evil.test/file",
            "https://user:pass@github.com/file",
            "https://github.com:444/file",
            "file:///tmp/file",
        ] {
            assert!(!trusted_url(url));
        }
    }
    #[test]
    fn package_selection_matches_published_names() {
        let mut value: Release = serde_json::from_slice(&release("v0.2.0")).unwrap();
        for suffix in [
            "windows-x64-setup.exe",
            "windows-x64-portable.zip",
            "linux-x64.tar.gz",
        ] {
            let name = format!("TurboDBNote-v0.2.0-{suffix}");
            value.assets.push(Asset {
                browser_download_url: format!("{RELEASES}/download/v0.2.0/{name}"),
                name,
                size: 20,
            });
        }
        for kind in [
            PackageKind::WindowsInstaller,
            PackageKind::WindowsPortable,
            PackageKind::Linux,
        ] {
            assert!(value.package(kind).is_ok());
        }
        value.assets[0].browser_download_url = "https://example.com/installer.exe".into();
        assert!(value.package(PackageKind::WindowsInstaller).is_err());
    }
    #[test]
    fn checksum_is_exact_unambiguous_and_supports_binary_marker() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{hash}  app.exe\n"), "app.exe").unwrap(),
            hash
        );
        assert_eq!(
            checksum(&format!("{hash} *app.exe\n"), "app.exe").unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash}  other-app.exe"), "app.exe").is_err());
        assert!(checksum(&format!("{hash}  app.exe\n{hash}  app.exe"), "app.exe").is_err());
        assert!(checksum(&format!("{}  app.exe", "z".repeat(64)), "app.exe").is_err());
    }
    #[test]
    fn stream_checks_hash_size_cancellation_and_disk_errors() {
        let payload = b"update package";
        let hash = format!("{:x}", Sha256::digest(payload));
        let cancel = AtomicBool::new(false);
        let run = |size, hash: &str| {
            stream(
                &mut &payload[..],
                &mut Vec::new(),
                size,
                hash,
                &cancel,
                &|_| {},
            )
        };
        assert!(run(payload.len() as u64, &hash).is_ok());
        assert_eq!(run(1, &hash).unwrap_err(), "updates.error.size");
        assert_eq!(run(100, &hash).unwrap_err(), "updates.error.size");
        assert_eq!(
            run(payload.len() as u64, &"0".repeat(64)).unwrap_err(),
            "updates.error.checksum"
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            run(payload.len() as u64, &hash).unwrap_err(),
            "updates.status.cancelled"
        );
        assert!(bounded(&payload[..], 2).is_err());
    }
    #[test]
    fn existing_download_is_reverified_and_tampering_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package");
        std::fs::write(&path, b"verified").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"verified"));
        let cancel = AtomicBool::new(false);
        verify(&path, &hash, &cancel).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert_eq!(
            verify(&path, &hash, &cancel).unwrap_err(),
            "updates.error.checksum"
        );
    }

    fn local_download_server(
        hash: String,
        payload: Vec<u8>,
        stalled: bool,
    ) -> (String, std::thread::JoinHandle<()>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let body = if index == 0 {
                    format!("{hash}  app.zip\n").into_bytes()
                } else {
                    payload.clone()
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                if index == 1 && stalled {
                    std::thread::sleep(Duration::from_millis(400));
                }
                let _ = socket.write_all(&body);
            }
        });
        (url, server)
    }
    #[test]
    fn local_http_download_publishes_only_verified_complete_packages() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let payload = vec![42; 128 * 1024];
        for mode in ["success", "bad_hash", "too_large", "cancel_stalled"] {
            let hash = if mode == "bad_hash" {
                "0".repeat(64)
            } else {
                format!("{:x}", Sha256::digest(&payload))
            };
            let (url, server) =
                local_download_server(hash.clone(), payload.clone(), mode == "cancel_stalled");
            let directory = tempfile::tempdir().unwrap();
            let asset = Asset {
                name: "app.zip".into(),
                browser_download_url: format!("{url}/package"),
                size: if mode == "too_large" {
                    1
                } else {
                    payload.len() as u64
                },
            };
            let cancel = AtomicBool::new(false);
            let started = Instant::now();
            let sums_url = format!("{url}/sums");
            let on_progress = |_: Progress| {
                if mode == "cancel_stalled" {
                    cancel.store(true, Ordering::Relaxed);
                }
            };
            let result = runtime.block_on(async {
                tokio::select! {
                    result = download_transfer(&asset, &sums_url, PackageKind::WindowsPortable, directory.path(), &cancel, &on_progress) => result,
                    _ = wait_for_cancel(&cancel) => Err("updates.status.cancelled".into()),
                }
            });
            if mode == "success" {
                let download = result.unwrap();
                assert_eq!(std::fs::read(&download.path).unwrap(), payload);
                assert_eq!(download.sha256, hash);
            } else {
                assert!(result.is_err());
                assert_eq!(
                    std::fs::read_dir(directory.path()).unwrap().count(),
                    0,
                    "no partial or invalid update is retained"
                );
                if mode == "cancel_stalled" {
                    assert!(started.elapsed() < Duration::from_millis(350));
                }
            }
            server.join().unwrap();
        }
    }
    #[test]
    fn download_rejects_redirects_to_untrusted_transport() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).unwrap();
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/untrusted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let asset = Asset {
            name: "app.zip".into(),
            browser_download_url: url.clone(),
            size: 1,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(download_transfer(
                    &asset,
                    &url,
                    PackageKind::WindowsPortable,
                    dir.path(),
                    &AtomicBool::new(false),
                    &|_| {}
                ))
                .is_err()
        );
        server.join().unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
