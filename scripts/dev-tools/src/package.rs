use std::{fs, path::Path, process::Command};
fn run(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|_| format!("Cannot start {program}"))?;
    if !output.status.success() {
        return Err(format!("{program} failed; diagnostic details omitted"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub fn linux(root: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("Linux packaging requires Linux".into());
    }
    let version = crate::release::version(root)?;
    crate::release::validate(root, &version)?;
    let name = format!("TurboDBNote-v{version}-linux-x64");
    let output = root.join("dist");
    let stage = output.join(&name);
    let archive = output.join(format!("{name}.tar.gz"));
    let checksum = output.join("SHA256SUMS-linux.txt");
    if stage.exists()
        || archive.exists()
        || checksum.exists()
        || output.join(format!("{name}-drivers.zip")).exists()
        || output.join(format!("{name}-minimal")).exists()
        || output.join(format!("{name}-minimal.tar.gz")).exists()
    {
        return Err("Refusing to overwrite an existing Linux package".into());
    }
    prefetch(root, "linux-x64", None, None)?;
    let args = [
        "build",
        "--locked",
        "--release",
        "-j",
        "1",
        "-p",
        "turbodbnote",
        "-p",
        "turbodbnote-driver-host",
        "--bins",
    ]
    .map(str::to_owned);
    if crate::cargo(&args)? != 0 {
        return Err("Native release build failed".into());
    }
    fs::create_dir_all(stage.join("drivers/native"))
        .map_err(|_| "Cannot create package directory")?;
    for (binary, target) in [
        ("turbodbnote", "turbodbnote"),
        (
            "turbodbnote-driver-host",
            "drivers/native/turbodbnote-driver-host",
        ),
    ] {
        let target = stage.join(target);
        fs::copy(root.join("target/release").join(binary), &target)
            .map_err(|_| "Cannot copy native binary")?;
        let dependencies = run(
            root,
            "ldd",
            &[target.to_str().ok_or("Invalid package path")?],
        )?;
        if dependencies.contains("not found") {
            return Err("Unresolved runtime dependency".into());
        }
        fs::write(
            stage.join(format!("{binary}-dependencies.txt")),
            dependencies,
        )
        .map_err(|_| "Cannot write dependency inventory")?;
    }
    fs::copy(
        root.join("LICENSE"),
        stage.join("drivers/native/LICENSE.TurboDBNote"),
    )
    .map_err(|_| "Cannot copy license")?;
    turbodbn_services::oracle_bundle::prepare(
        &root.join("target/tools/oracle-bundle"),
        &stage.join("drivers/native"),
        "linux-x64",
        None,
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let driver_archive = output.join(format!("{name}-drivers.zip"));
    turbodbn_services::driver_packages::build(
        &stage.join("drivers/native"),
        &driver_archive,
        &version,
        "linux-x64",
    )?;
    fs::write(stage.join("package-flavor.txt"), "full\n")
        .map_err(|_| "Cannot write package flavor")?;
    for file in ["LICENSE", "README.md", "README.zh-CN.md"] {
        fs::copy(root.join(file), stage.join(file)).map_err(|_| "Cannot copy documentation")?;
    }
    for (language, file) in [
        ("en", "RELEASE-NOTES.md"),
        ("zh-CN", "RELEASE-NOTES.zh-CN.md"),
    ] {
        fs::write(
            stage.join(file),
            crate::release::notes(root, &version, language)?,
        )
        .map_err(|_| "Cannot write release notes")?;
    }
    fs::write(stage.join("README.txt"),"Run ./turbodbnote after extraction. Linux x64, Ubuntu 22.04/glibc 2.35 baseline.\nRequires X11/Wayland, Vulkan graphics, libxkbcommon, fontconfig. Extension workers additionally require unixODBC; Oracle requires libaio.\nPasswords are session-only on Linux. Windows tray and legacy MySQL DLL are Windows-only.\nApplication data uses the per-user config directory. AI requires a separately installed CLI.\nSee RELEASE-NOTES.md and dependency inventories.\n").map_err(|_|"Cannot write package instructions")?;
    let commit = run(root, "git", &["rev-parse", "HEAD"])?;
    fs::write(stage.join("BUILD-INFO.json"),serde_json::to_vec_pretty(&serde_json::json!({"version":version,"commit":commit.trim(),"target":"x86_64-unknown-linux-gnu"})).map_err(|_|"Cannot encode build info")?).map_err(|_|"Cannot write build info")?;
    let sources = format!("--output={}", stage.join("sources.tar.gz").display());
    run(
        root,
        "git",
        &[
            "archive",
            "--format=tar.gz",
            &sources,
            "HEAD",
            "Cargo.toml",
            "Cargo.lock",
            "crates",
            "vendor/gpui-component",
            "scripts",
            "LICENSE",
            "README.md",
            "README.zh-CN.md",
            "CHANGELOG.md",
            "CHANGELOG.zh-CN.md",
        ],
    )?;
    run(
        root,
        "tar",
        &[
            "-czf",
            archive.to_str().ok_or("Invalid archive path")?,
            "-C",
            output.to_str().ok_or("Invalid output path")?,
            &name,
        ],
    )?;
    let minimal_name = format!("{name}-minimal");
    let minimal_stage = output.join(&minimal_name);
    if minimal_stage.exists() {
        return Err("Refusing to overwrite minimal package".into());
    }
    fs::create_dir(&minimal_stage).map_err(|_| "Cannot create minimal package")?;
    for entry in fs::read_dir(&stage).map_err(|_| "Cannot read package")? {
        let entry = entry.map_err(|_| "Cannot read package")?;
        if entry
            .file_type()
            .map_err(|_| "Cannot read package")?
            .is_file()
        {
            fs::copy(entry.path(), minimal_stage.join(entry.file_name()))
                .map_err(|_| "Cannot copy minimal package")?;
        }
    }
    fs::write(minimal_stage.join("package-flavor.txt"), "minimal\n")
        .map_err(|_| "Cannot write package flavor")?;
    let minimal_archive = output.join(format!("{minimal_name}.tar.gz"));
    run(
        root,
        "tar",
        &[
            "-czf",
            minimal_archive.to_str().ok_or("Invalid path")?,
            "-C",
            output.to_str().ok_or("Invalid path")?,
            &minimal_name,
        ],
    )?;
    let mut sums = String::new();
    for file in [&archive, &minimal_archive, &driver_archive] {
        let digest = turbodbn_services::driver_packages::hash(
            file,
            &std::sync::atomic::AtomicBool::new(false),
        )?;
        sums.push_str(&format!(
            "{digest}  {}\n",
            file.file_name().ok_or("Invalid path")?.to_string_lossy()
        ));
    }
    fs::write(checksum, sums).map_err(|_| "Cannot write checksums")?;
    println!("{name}.tar.gz");
    Ok(())
}

pub fn prefetch(
    root: &Path,
    platform: &str,
    oracle: Option<&Path>,
    db2: Option<&Path>,
) -> Result<(), String> {
    let stage = tempfile::tempdir().map_err(|_| "Cannot stage client preflight")?;
    let flag = std::sync::atomic::AtomicBool::new(false);
    turbodbn_services::oracle_bundle::prepare(
        &root.join("target/tools/oracle-bundle"),
        stage.path(),
        platform,
        oracle,
        &flag,
    )?;
    if platform == "windows-x64" {
        turbodbn_services::db2_bundle::prepare(
            &root.join("target/tools/db2-client"),
            stage.path(),
            db2,
            &flag,
        )?;
    }
    Ok(())
}
