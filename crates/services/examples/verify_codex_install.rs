//! Only installs into the fixed GUI verification fixture; never logs in or sends prompts.
use std::sync::{Mutex, atomic::AtomicBool};
use turbodbn_services::codex_install;
fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gui-verification/sidebar-codex/data/agents/codex");
    let last = Mutex::new(0u64);
    let result = codex_install::install(&root, None, &AtomicBool::new(false), |progress| {
        let mib = progress.bytes / 1024 / 1024;
        let mut previous = last.lock().unwrap();
        if mib >= *previous + 20 || progress.verifying {
            println!("Codex package: {mib} MiB; verifying={}", progress.verifying);
            *previous = mib;
        }
    });
    match result {
        Ok(candidate) => {
            assert!(candidate.version == format!("codex-cli {}", codex_install::VERSION));
            println!(
                "PASS: official package hash, platform, version and App Server capability; no login or inference"
            );
        }
        Err(error) => {
            eprintln!("Codex installation verification: {error}");
            std::process::exit(1);
        }
    }
}
