//! Verified build-time downloads. Never expose raw network errors or response bodies.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
pub struct Archive {
    pub component: &'static str,
    pub url: &'static str,
    pub name: &'static str,
    pub sha256: &'static str,
    pub size: Option<u64>,
    pub offline: &'static str,
}
impl Archive {
    fn failure(&self, stage: &str) -> String {
        format!(
            "{}: {}; provide the official ZIP using {}",
            self.component, stage, self.offline
        )
    }
    fn verify(&self, path: &Path, flag: &AtomicBool) -> Result<(), String> {
        if flag.load(Ordering::Relaxed) {
            return Err("components.cancelled".into());
        }
        let size = fs::metadata(path)
            .map_err(|_| self.failure("archive unreadable"))?
            .len();
        if size > 256 * 1024 * 1024
            || self.size.is_some_and(|expected| size != expected)
            || crate::driver_packages::hash(path, flag)? != self.sha256
        {
            return Err(self.failure("SHA256 or size mismatch; replace the cached/offline ZIP"));
        }
        Ok(())
    }
    pub fn acquire(
        &self,
        cache: &Path,
        supplied: Option<&Path>,
        flag: &AtomicBool,
    ) -> Result<PathBuf, String> {
        if let Some(path) = supplied {
            self.verify(path, flag)?;
            return Ok(path.to_owned());
        }
        fs::create_dir_all(cache).map_err(|_| self.failure("cannot create cache"))?;
        let cached = cache.join(self.name);
        if cached.exists() {
            self.verify(&cached, flag)?;
            println!("{}: verified cache", self.component);
            return Ok(cached);
        }
        let host = reqwest::Url::parse(self.url)
            .map_err(|_| self.failure("invalid source"))?
            .host_str()
            .ok_or_else(|| self.failure("invalid host"))?
            .to_owned();
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .user_agent("TurboDBNote-build/0.1")
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() < 3
                    && attempt.url().scheme() == "https"
                    && attempt.url().host_str() == Some(host.as_str())
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|_| self.failure("HTTP client initialization failed"))?;
        for attempt in 1..=3 {
            if flag.load(Ordering::Relaxed) {
                return Err("components.cancelled".into());
            }
            println!("{}: download attempt {attempt}/3", self.component);
            let result = self.download(&client, cache, flag);
            match result {
                Ok(temp) => {
                    self.verify(temp.path(), flag)?;
                    temp.persist_noclobber(&cached)
                        .map_err(|_| self.failure("cannot publish cache"))?;
                    println!("{}: download verified", self.component);
                    return Ok(cached);
                }
                Err((reason, retry)) => {
                    let message = self.failure(&reason);
                    if !retry || attempt == 3 {
                        return Err(message);
                    }
                    println!("{message}; retrying");
                    for _ in 0..attempt * 10 {
                        if flag.load(Ordering::Relaxed) {
                            return Err("components.cancelled".into());
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        }
        unreachable!()
    }
    fn download(
        &self,
        client: &reqwest::blocking::Client,
        cache: &Path,
        flag: &AtomicBool,
    ) -> Result<tempfile::NamedTempFile, (String, bool)> {
        let mut response = client.get(self.url).send().map_err(|e| {
            let retry = e.is_timeout() || e.is_connect();
            (
                if e.is_timeout() {
                    "request timed out"
                } else if e.is_connect() {
                    "connection failed"
                } else {
                    "request failed"
                }
                .into(),
                retry,
            )
        })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err((format!("HTTP {status}"), retry_status(status)));
        }
        let mut temp = tempfile::NamedTempFile::new_in(cache)
            .map_err(|_| ("cannot create temporary archive".into(), false))?;
        let mut buffer = [0; 65536];
        let mut total = 0u64;
        loop {
            if flag.load(Ordering::Relaxed) {
                return Err(("cancelled".into(), false));
            }
            let n = response
                .read(&mut buffer)
                .map_err(|_| ("response interrupted".into(), true))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > self.size.unwrap_or(256 * 1024 * 1024) {
                return Err(("archive exceeds expected size".into(), false));
            }
            temp.write_all(&buffer[..n])
                .map_err(|_| ("cannot write archive".into(), false))?;
        }
        temp.as_file()
            .sync_all()
            .map_err(|_| ("cannot flush archive".into(), false))?;
        Ok(temp)
    }
}
fn retry_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_transient_statuses_retry() {
        for s in [429, 500, 502, 503, 504] {
            assert!(retry_status(s));
        }
        for s in [200, 301, 403, 404, 501] {
            assert!(!retry_status(s));
        }
    }
    #[test]
    fn transient_response_retries_and_publishes_only_verified_bytes() {
        use std::{net::TcpListener, thread};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url: &'static str = Box::leak(
            format!("http://{}/fixture", listener.local_addr().unwrap()).into_boxed_str(),
        );
        let server = thread::spawn(move || {
            for reply in [
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                assert!(count > 0);
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let a = Archive {
            component: "Fixture",
            url,
            name: "test.zip",
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            size: Some(3),
            offline: "-FixtureArchive",
        };
        let flag = AtomicBool::new(false);
        let path = a.acquire(dir.path(), None, &flag).unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        assert_eq!(a.acquire(dir.path(), None, &flag).unwrap(), path);
    }
    #[test]
    fn bad_cache_and_offline_archive_fail_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.zip");
        fs::write(&path, b"bad").unwrap();
        let a = Archive {
            component: "Fixture",
            url: "https://invalid.example/file",
            name: "test.zip",
            sha256: "invalid",
            size: Some(3),
            offline: "-FixtureArchive",
        };
        let flag = AtomicBool::new(false);
        assert!(
            a.acquire(dir.path(), None, &flag)
                .unwrap_err()
                .contains("SHA256")
        );
        assert!(a.acquire(dir.path(), Some(&path), &flag).is_err());
        assert_eq!(fs::read(path).unwrap(), b"bad");
    }
}
