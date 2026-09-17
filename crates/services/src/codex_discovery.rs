//! Read-only PATH / npm / Volta discovery. Never execute shell launchers.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub source: String,
    pub version: String,
}

fn package_binaries(package: &Path) -> Vec<PathBuf> {
    let (platform, triple, exe) = if cfg!(target_os = "windows") {
        if cfg!(target_arch = "aarch64") {
            ("win32-arm64", "aarch64-pc-windows-msvc", "codex.exe")
        } else {
            ("win32-x64", "x86_64-pc-windows-msvc", "codex.exe")
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            ("darwin-arm64", "aarch64-apple-darwin", "codex")
        } else {
            ("darwin-x64", "x86_64-apple-darwin", "codex")
        }
    } else if cfg!(target_arch = "aarch64") {
        ("linux-arm64", "aarch64-unknown-linux-musl", "codex")
    } else {
        ("linux-x64", "x86_64-unknown-linux-musl", "codex")
    };
    let mut paths = vec![];
    for base in [
        package.to_path_buf(),
        package.join(format!("node_modules/@openai/codex-{platform}")),
        package
            .parent()
            .unwrap_or(package)
            .join(format!("codex-{platform}")),
    ] {
        for dir in ["bin", "codex"] {
            paths.push(base.join("vendor").join(triple).join(dir).join(exe));
        }
    }
    paths
}

fn collect(paths: impl IntoIterator<Item = PathBuf>, fallback: bool) -> Vec<(PathBuf, String)> {
    let mut output = vec![];
    let mut seen = HashSet::new();
    for directory in paths.into_iter().filter(|p| p.is_absolute()).take(128) {
        let executable = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        let mut candidates = vec![(executable, "PATH")];
        let launcher = directory.join("codex.cmd").is_file()
            || directory.join("codex.ps1").is_file()
            || directory.join("codex").is_file();
        if launcher || fallback {
            for package in [
                directory.join("node_modules/@openai/codex"),
                directory.join("../lib/node_modules/@openai/codex"),
            ] {
                candidates.extend(package_binaries(&package).into_iter().map(|p| (p, "npm")));
            }
            // Volta's global package image is authoritative before node-version fallbacks.
            let volta = directory.parent().unwrap_or(&directory);
            let package =
                volta.join("tools/image/packages/@openai/codex/node_modules/@openai/codex");
            candidates.extend(package_binaries(&package).into_iter().map(|p| (p, "Volta")));
        }
        for (candidate, source) in candidates {
            if candidate.is_file()
                && let Ok(path) = dunce::canonicalize(&candidate)
            {
                let key = if cfg!(windows) {
                    path.to_string_lossy().to_lowercase()
                } else {
                    path.to_string_lossy().into_owned()
                };
                if seen.insert(key) {
                    output.push((path, source.into()));
                }
            }
        }
    }
    output
}

async fn version(path: &Path, timeout: Duration) -> Option<String> {
    version_for(path, timeout, false).await
}
async fn version_for(path: &Path, timeout: Duration, claude: bool) -> Option<String> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?.take(4097);
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = vec![];
        stdout.read_to_end(&mut bytes).await.ok()?;
        if bytes.len() > 4096 || !child.wait().await.ok()?.success() {
            return None;
        }
        let text = String::from_utf8(bytes).ok()?;
        let text = text.trim();
        (if claude {
            text.contains("(Claude Code)")
        } else {
            text.starts_with("codex-cli ")
        })
        .then(|| text.chars().take(120).collect())
    })
    .await
    .ok()
    .flatten();
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

pub fn discover_claude() -> Vec<Candidate> {
    let mut roots = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    for var in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(var) {
            roots.push(PathBuf::from(home).join(".local/bin"));
        }
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return vec![];
    };
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        let mut seen = HashSet::new();
        let mut found = vec![];
        for root in roots.into_iter().filter(|p| p.is_absolute()).take(128) {
            let path = root.join(if cfg!(windows) {
                "claude.exe"
            } else {
                "claude"
            });
            if !path.is_file() {
                continue;
            }
            let Ok(path) = dunce::canonicalize(path) else {
                continue;
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
            if budget.is_zero() {
                break;
            }
            if let Some(version) =
                version_for(&path, budget.min(Duration::from_secs(2)), true).await
            {
                found.push(Candidate {
                    path,
                    source: "Claude 原生安装 / PATH".into(),
                    version,
                });
            }
        }
        found
    })
}

/// Runs on a background executor. Total subprocess verification budget: 6 seconds.
pub fn discover() -> Vec<Candidate> {
    let paths = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default();
    let locations = collect(paths, false);
    let fallback = || {
        let mut roots = vec![];
        if let Some(path) = std::env::var_os("VOLTA_HOME") {
            roots.push(PathBuf::from(path).join("bin"));
        }
        if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(path).join("Volta/bin"));
        }
        if let Some(path) = std::env::var_os("APPDATA") {
            roots.push(PathBuf::from(path).join("npm"));
        }
        if let Some(path) = std::env::var_os("NPM_CONFIG_PREFIX") {
            roots.push(PathBuf::from(path));
        }
        collect(roots, true)
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return vec![];
    };
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        let mut found = vec![];
        let mut seen = HashSet::new();
        for group in 0..2 {
            let current = if group == 0 {
                locations.clone()
            } else {
                fallback()
            };
            for (path, source) in current.into_iter().take(16) {
                if !seen.insert(path.clone()) {
                    continue;
                }
                let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
                if budget.is_zero() {
                    break;
                }
                if let Some(version) = version(&path, budget.min(Duration::from_secs(2))).await {
                    found.push(Candidate {
                        path,
                        source,
                        version,
                    });
                }
            }
            if !found.is_empty() || tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        found
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn includes_nested_and_legacy_npm_layouts() {
        let paths = package_binaries(Path::new("package"));
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().contains("node_modules"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.components().any(|c| c.as_os_str() == "vendor"))
        );
        assert_eq!(paths.len(), 6);
    }
    #[test]
    fn ignores_relative_search_roots() {
        assert!(collect([PathBuf::from(".")], true).is_empty());
    }
    #[test]
    fn resolves_volta_launcher_without_executing_it_and_deduplicates() {
        let root =
            std::env::temp_dir().join(format!("turbodbnote-discovery-{}", uuid::Uuid::new_v4()));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            bin.join("codex.cmd"),
            "this launcher must never be executed",
        )
        .unwrap();
        let package = root.join("tools/image/packages/@openai/codex/node_modules/@openai/codex");
        let binary = package_binaries(&package)[0].clone();
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"placeholder").unwrap();
        let found = collect([bin.clone(), bin], false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, dunce::canonicalize(binary).unwrap());
        assert_eq!(found[0].1, "Volta");
        std::fs::remove_dir_all(root).unwrap();
    }
}
