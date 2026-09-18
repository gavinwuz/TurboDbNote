use std::path::PathBuf;
use turbodbn_diagnostics::{Guard, config::Config};

pub fn start() -> Option<Guard> {
    let exe = std::env::current_exe().ok()?;
    let directory = exe.parent()?;
    let settings = crate::preferences::Preferences::path().ok()?;
    let data = settings.parent()?.join("diagnostics");
    // Optional local deployment override, separate from user model/API credentials.
    let override_path = directory.join("crashsight.local.json");
    let bytes = if override_path.is_file() {
        match read_config(&override_path) {
            Ok(bytes) => bytes,
            Err(()) => {
                eprintln!("Invalid local diagnostics configuration; diagnostics disabled");
                return None;
            }
        }
    } else {
        include_bytes!("../assets/crashsight.json").to_vec()
    };
    let config = match Config::from_json(&bytes) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return None;
        }
    };
    Some(turbodbn_diagnostics::start(
        config,
        directory,
        &data,
        env!("CARGO_PKG_VERSION"),
    ))
}
fn read_config(path: &PathBuf) -> Result<Vec<u8>, ()> {
    use std::io::Read;
    let mut bytes = vec![];
    std::fs::File::open(path)
        .map_err(|_| ())?
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > 16_384 {
        return Err(());
    }
    Ok(bytes)
}
