use chrono::{FixedOffset, NaiveDate, Utc};
use regex::Regex;
use std::{fs, path::Path};

fn read(root: &Path, file: &str) -> Result<String, String> {
    fs::read_to_string(root.join(file)).map_err(|_| format!("Cannot read release input: {file}"))
}
fn toml(root: &Path, file: &str) -> Result<toml::Value, String> {
    read(root, file)?
        .parse()
        .map_err(|_| format!("Invalid TOML: {file}"))
}
pub fn version(root: &Path) -> Result<String, String> {
    toml(root, "Cargo.toml")?["workspace"]["package"]["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or("Missing workspace version".into())
}
pub fn notes(root: &Path, version: &str, language: &str) -> Result<String, String> {
    let text = read(
        root,
        if language == "en" {
            "CHANGELOG.md"
        } else {
            "CHANGELOG.zh-CN.md"
        },
    )?;
    let heading = format!("## [{version}]");
    let mut sections = Vec::new();
    let mut body = None::<String>;
    for line in text.lines() {
        if line.starts_with("## ") {
            if let Some(value) = body.take() {
                sections.push(value);
            }
            if line
                .strip_prefix(&heading)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(' '))
            {
                body = Some(String::new());
            }
        } else if let Some(body) = &mut body {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(value) = body {
        sections.push(value);
    }
    if sections.len() != 1 || sections[0].trim().is_empty() {
        return Err("Expected one nonempty changelog section for this version".into());
    }
    Ok(format!("# v{version}\n\n{}\n", sections[0].trim()))
}
pub fn validate(root: &Path, requested: &str) -> Result<serde_json::Value, String> {
    if !Regex::new(r"^\d+\.\d+\.\d+$").unwrap().is_match(requested) {
        return Err("Version must be x.y.z".into());
    }
    let metadata: serde_json::Value =
        serde_json::from_str(&read(root, "crates/desktop/assets/release.json")?)
            .map_err(|_| "Invalid release metadata")?;
    if version(root)? != requested
        || metadata["version"] != requested
        || metadata["channel"] != "stable"
    {
        return Err("Cargo and stable release metadata must match".into());
    }
    NaiveDate::parse_from_str(
        metadata["released_on"]
            .as_str()
            .ok_or("Missing release date")?,
        "%Y-%m-%d",
    )
    .map_err(|_| "Invalid release date")?;
    let manifest = toml(root, "Cargo.toml")?;
    let lock = toml(root, "Cargo.lock")?;
    let packages = lock["package"]
        .as_array()
        .ok_or("Missing locked packages")?;
    for member in manifest["workspace"]["members"]
        .as_array()
        .ok_or("Missing workspace members")?
    {
        let member = member.as_str().ok_or("Invalid workspace member")?;
        let package = toml(root, &format!("{member}/Cargo.toml"))?;
        let name = package["package"]["name"]
            .as_str()
            .ok_or("Missing package name")?;
        let found: Vec<_> = packages
            .iter()
            .filter(|p| p["name"].as_str() == Some(name))
            .collect();
        if found.len() != 1 || found[0]["version"].as_str() != Some(requested) {
            return Err("Workspace package versions in Cargo.lock must match".into());
        }
    }
    let resource = read(root, "crates/desktop/assets/windows.rc")?;
    let numeric = format!("{},0", requested.replace('.', ","));
    for key in ["FILEVERSION", "PRODUCTVERSION"] {
        if !resource.contains(&format!("{key} {numeric}")) {
            return Err("Windows numeric version resources must match".into());
        }
    }
    for key in ["FileVersion", "ProductVersion"] {
        if !resource.contains(&format!("VALUE \"{key}\", \"{requested}\\0\"")) {
            return Err("Windows string version resources must match".into());
        }
    }
    notes(root, requested, "en")?;
    notes(root, requested, "zh-CN")?;
    Ok(metadata)
}
pub fn validate_cli(root: &Path, args: &[String]) -> Result<(), String> {
    let mut requested = None;
    let mut output = None;
    let mut publication = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--version" | "--notes" => {
                let value = args.get(i + 1).ok_or("Missing option value")?;
                if args[i] == "--version" {
                    requested = Some(value);
                } else {
                    output = Some(value);
                }
                i += 2;
            }
            "--publication" => {
                publication = true;
                i += 1;
            }
            _ => return Err("Unknown release option".into()),
        }
    }
    let requested = requested.ok_or("--version is required")?;
    let metadata = validate(root, requested)?;
    if publication
        && metadata["released_on"]
            != Utc::now()
                .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
                .date_naive()
                .to_string()
    {
        return Err("Release date differs from publication day; prepare a new candidate".into());
    }
    if let Some(output) = output {
        fs::write(
            output,
            format!(
                "{}\n---\n\n{}",
                notes(root, requested, "en")?,
                notes(root, requested, "zh-CN")?
            ),
        )
        .map_err(|_| "Cannot write release notes")?;
    }
    println!("Validated v{requested}");
    Ok(())
}
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|_| "Cannot start Git")?;
    if !output.status.success() {
        return Err("Git operation failed; diagnostic details omitted".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
pub fn prepare(root: &Path, args: &[String]) -> Result<(), String> {
    let requested = args
        .first()
        .filter(|_| args.len() == 1)
        .ok_or("prepare-release requires one x.y.z version")?;
    if !Regex::new(r"^\d+\.\d+\.\d+$").unwrap().is_match(requested) {
        return Err("Version must be x.y.z".into());
    }
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        return Err("Commit work first; release preparation requires a clean worktree".into());
    }
    let refs = git(root, &["ls-remote", "--symref", "origin", "HEAD"])?;
    let branch = refs
        .lines()
        .find_map(|l| {
            l.strip_prefix("ref: refs/heads/")
                .and_then(|s| s.split_whitespace().next())
        })
        .ok_or("Cannot identify default branch")?;
    let head = refs
        .lines()
        .find(|l| l.ends_with("\tHEAD") && !l.starts_with("ref:"))
        .and_then(|l| l.split_whitespace().next())
        .ok_or("Missing remote HEAD")?;
    if git(root, &["branch", "--show-current"])? != branch
        || git(root, &["rev-parse", "HEAD"])? != head
    {
        return Err("Clean default branch must match origin before preparing a release".into());
    }
    let old = version(root)?;
    let numbers = |v: &str| -> Result<Vec<u64>, String> {
        v.split('.')
            .map(|n| n.parse().map_err(|_| "Invalid numeric version".into()))
            .collect()
    };
    if numbers(requested)? <= numbers(&old)? {
        return Err("Version must increase; published versions are immutable".into());
    }
    if !git(
        root,
        &[
            "ls-remote",
            "--tags",
            "origin",
            &format!("refs/tags/v{requested}"),
        ],
    )?
    .is_empty()
    {
        return Err("Version tag already exists".into());
    }
    let date = Utc::now()
        .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
        .date_naive()
        .to_string();
    let mut edits = Vec::new();
    let heading = Regex::new(r"(?m)^## \[Unreleased\][^\n]*\n").unwrap();
    let next_heading = Regex::new(r"(?m)^## ").unwrap();
    for file in ["CHANGELOG.md", "CHANGELOG.zh-CN.md"] {
        let text = read(root, file)?;
        let found = heading.find(&text).ok_or("Missing Unreleased section")?;
        let rest = &text[found.end()..];
        let end = next_heading.find(rest).map_or(rest.len(), |m| m.start());
        if rest[..end].trim().is_empty() {
            return Err(format!("Add release notes to Unreleased in {file}"));
        }
        edits.push((
            file.to_owned(),
            format!(
                "{}## [Unreleased]\n\n## [{requested}] - {date}\n\n{}\n\n{}",
                &text[..found.start()],
                rest[..end].trim(),
                &rest[end..]
            ),
        ));
    }
    let manifest = read(root, "Cargo.toml")?;
    let pattern = Regex::new(&format!(
        r#"(\[workspace\.package\]\s*version\s*=\s*"){}(")"#,
        regex::escape(&old)
    ))
    .unwrap();
    if !pattern.is_match(&manifest) {
        return Err("Cannot locate workspace version".into());
    }
    edits.push((
        "Cargo.toml".into(),
        pattern
            .replace(&manifest, |caps: &regex::Captures| {
                format!("{}{requested}{}", &caps[1], &caps[2])
            })
            .into_owned(),
    ));
    let workspace = toml(root, "Cargo.toml")?;
    let mut own = Vec::new();
    for member in workspace["workspace"]["members"]
        .as_array()
        .ok_or("Missing members")?
    {
        let package = toml(
            root,
            &format!("{}/Cargo.toml", member.as_str().ok_or("Invalid member")?),
        )?;
        own.push(
            package["package"]["name"]
                .as_str()
                .ok_or("Missing package name")?
                .to_owned(),
        );
    }
    let lock = read(root, "Cargo.lock")?;
    let names = Regex::new(r#"(?m)^\s*name\s*=\s*"([^"]+)""#).unwrap();
    let versions = Regex::new(&format!(
        r#"(?m)(^\s*version\s*=\s*"){}(")"#,
        regex::escape(&old)
    ))
    .unwrap();
    let blocks: Vec<_> = lock
        .split("[[package]]")
        .map(|block| {
            if names
                .captures(block)
                .is_some_and(|c| own.iter().any(|name| name == &c[1]))
            {
                versions
                    .replace(block, |caps: &regex::Captures| {
                        format!("{}{requested}{}", &caps[1], &caps[2])
                    })
                    .into_owned()
            } else {
                block.to_owned()
            }
        })
        .collect();
    edits.push(("Cargo.lock".into(), blocks.join("[[package]]")));
    let mut resource = read(root, "crates/desktop/assets/windows.rc")?;
    for key in ["FILEVERSION", "PRODUCTVERSION"] {
        resource = Regex::new(&format!(r"{key} \d+,\d+,\d+,\d+"))
            .unwrap()
            .replace_all(
                &resource,
                format!("{key} {},0", requested.replace('.', ",")),
            )
            .into_owned();
    }
    resource = resource.replace(&format!("\"{old}\\0\""), &format!("\"{requested}\\0\""));
    edits.push(("crates/desktop/assets/windows.rc".into(), resource));
    edits.push((
        "crates/desktop/assets/release.json".into(),
        format!(
            "{}\n",
            serde_json::to_string_pretty(
                &serde_json::json!({"version":requested,"channel":"stable","released_on":date})
            )
            .map_err(|_| "Cannot encode release metadata")?
        ),
    ));
    for file in ["README.md", "README.zh-CN.md"] {
        edits.push((
            file.into(),
            read(root, file)?
                .replacen(&format!("**v{old}**"), &format!("**v{requested}**"), 1)
                .replacen(
                    &format!("**Version {old} ·"),
                    &format!("**Version {requested} ·"),
                    1,
                )
                .replacen(
                    &format!("**版本 {old} ·"),
                    &format!("**版本 {requested} ·"),
                    1,
                ),
        ));
    }
    for (file, text) in &edits {
        fs::write(root.join(file), text).map_err(|_| "Cannot write prepared release")?;
    }
    validate(root, requested)?;
    git(root, &["config", "--local", "user.name", "gavinwuz"])?;
    let mut add = vec!["add"];
    add.extend(edits.iter().map(|(file, _)| file.as_str()));
    git(root, &add)?;
    git(
        root,
        &[
            "commit",
            "-m",
            &format!("chore: prepare v{requested} release [release]"),
        ],
    )?;
    git(
        root,
        &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
    )?;
    println!(
        "Dual-platform validation and publication triggered; no tag is created before packages pass."
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_checksums_use_portable_lf_and_preserve_spaces_in_filenames() {
        use sha2::{Digest, Sha256};
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("package with spaces.zip");
        let output = directory.path().join("SHA256SUMS.txt");
        fs::write(&file, b"isolated archive fixture").unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../release-notes.ps1");
        let invocation = directory.path().join("checksums.ps1");
        fs::write(&invocation,"param($Library,$Archive,$OutputFile)\n. $Library\nWrite-ReleaseChecksums -Paths @($Archive) -OutputFile $OutputFile\n").unwrap();
        let status = std::process::Command::new("pwsh")
            .args(["-NoProfile", "-File"])
            .arg(&invocation)
            .arg(&script)
            .arg(&file)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "Checksum helper failed; diagnostic details omitted"
        );
        assert_eq!(
            fs::read(output).unwrap(),
            format!(
                "{:x}  package with spaces.zip\n",
                Sha256::digest(b"isolated archive fixture")
            )
            .as_bytes()
        );
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("crates/desktop/assets")).unwrap();
        fs::create_dir_all(root.join("crates/core")).unwrap();
        for (file, text) in [
            (
                "Cargo.toml",
                "[workspace]\nmembers=[\"crates/core\"]\n[workspace.package]\nversion=\"0.1.0\"\n",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname=\"sample\"\nversion=\"0.1.0\"\n",
            ),
            ("crates/core/Cargo.toml", "[package]\nname=\"sample\"\n"),
            (
                "crates/desktop/assets/release.json",
                r#"{"version":"0.1.0","channel":"stable","released_on":"2026-10-12"}"#,
            ),
            (
                "crates/desktop/assets/windows.rc",
                "FILEVERSION 0,1,0,0\nPRODUCTVERSION 0,1,0,0\nVALUE \"FileVersion\", \"0.1.0\\0\"\nVALUE \"ProductVersion\", \"0.1.0\\0\"",
            ),
            (
                "CHANGELOG.md",
                "## [Unreleased]\n\n## [0.1.0] - 2026-10-12\n\nFeature\n",
            ),
            ("CHANGELOG.zh-CN.md", "## [0.1.0]\n\n功能\n"),
        ] {
            fs::write(root.join(file), text).unwrap();
        }
        dir
    }
    #[test]
    fn validates_all_workspace_members_and_both_changelogs() {
        let dir = fixture();
        assert!(validate(dir.path(), "0.1.0").is_ok());
        fs::write(
            dir.path().join("Cargo.lock"),
            "[[package]]\nname=\"sample\"\nversion=\"0.0.9\"\n",
        )
        .unwrap();
        assert!(validate(dir.path(), "0.1.0").is_err());
    }
    #[test]
    fn duplicate_and_empty_chapters_are_rejected() {
        let dir = fixture();
        fs::write(
            dir.path().join("CHANGELOG.md"),
            "## [0.1.0]\na\n## [0.1.0]\nb\n",
        )
        .unwrap();
        assert!(notes(dir.path(), "0.1.0", "en").is_err());
        fs::write(dir.path().join("CHANGELOG.md"), "## [0.1.0]\n\n").unwrap();
        assert!(notes(dir.path(), "0.1.0", "en").is_err());
    }
    #[test]
    fn preparation_preserves_dependencies_and_published_notes_in_isolated_local_git() {
        let dir = fixture();
        let root = dir.path();
        let remote = tempfile::tempdir().unwrap();
        fs::write(root.join("README.md"), "**Version 0.1.0 · Windows**\n").unwrap();
        fs::write(root.join("README.zh-CN.md"), "**版本 0.1.0 · Windows**\n").unwrap();
        let original = read(root, "Cargo.toml").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            format!("{original}\n[workspace.dependencies]\nexample = {{ version = \"0.1.0\" }}\n"),
        )
        .unwrap();
        for file in ["CHANGELOG.md", "CHANGELOG.zh-CN.md"] {
            let previous = read(root, file).unwrap().replace("## [Unreleased]\n\n", "");
            fs::write(
                root.join(file),
                format!("## [Unreleased]\n\nNew feature\n\n{previous}"),
            )
            .unwrap();
        }
        git(remote.path(), &["init", "--bare", "-b", "main"]).unwrap();
        git(root, &["init", "-b", "main"]).unwrap();
        git(root, &["config", "--local", "user.name", "gavinwuz"]).unwrap();
        git(
            root,
            &["config", "--local", "user.email", "fixture@example.invalid"],
        )
        .unwrap();
        git(root, &["config", "--local", "commit.gpgsign", "false"]).unwrap();
        let hooks = root.join(".git/fixture-hooks");
        fs::create_dir_all(&hooks).unwrap();
        git(
            root,
            &[
                "config",
                "--local",
                "core.hooksPath",
                hooks.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(root, &["add", "."]).unwrap();
        git(root, &["commit", "-m", "test: seed release fixture"]).unwrap();
        git(
            root,
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        )
        .unwrap();
        git(root, &["push", "origin", "main"]).unwrap();
        let prepared = prepare(root, &["0.1.1".into()]);
        assert_eq!(
            toml(root, "Cargo.lock").unwrap()["package"][0]["version"].as_str(),
            Some("0.1.1")
        );
        prepared.unwrap();
        assert!(validate(root, "0.1.1").is_ok());
        assert!(
            read(root, "Cargo.toml")
                .unwrap()
                .contains("example = { version = \"0.1.0\" }")
        );
        assert!(notes(root, "0.1.0", "en").unwrap().contains("Feature"));
        assert!(git(root, &["status", "--porcelain"]).unwrap().is_empty());
        assert!(git(root, &["tag"]).unwrap().is_empty());
        assert_eq!(
            git(root, &["rev-parse", "HEAD"]).unwrap(),
            git(remote.path(), &["rev-parse", "refs/heads/main"]).unwrap()
        );
    }
}
