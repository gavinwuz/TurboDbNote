use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path, process::Command};
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
    if stage.exists() || archive.exists() || checksum.exists() {
        return Err("Refusing to overwrite an existing Linux package".into());
    }
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
    fs::write(stage.join("README.txt"),"Run ./turbodbnote after extraction. Linux x64, Ubuntu 22.04/glibc 2.35 baseline.\nRequires X11/Wayland, Vulkan graphics, libxkbcommon, fontconfig and unixODBC.\nPasswords are session-only on Linux. Windows tray and legacy MySQL DLL are Windows-only.\nApplication data uses the per-user config directory. AI requires a separately installed CLI.\nSee RELEASE-NOTES.md and dependency inventories.\n").map_err(|_|"Cannot write package instructions")?;
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
    let mut source = fs::File::open(&archive).map_err(|_| "Cannot open archive")?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = source
            .read(&mut buffer)
            .map_err(|_| "Cannot read archive")?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    fs::write(checksum, format!("{:x}  {name}.tar.gz\n", hash.finalize()))
        .map_err(|_| "Cannot write checksum")?;
    println!("{name}.tar.gz");
    Ok(())
}
