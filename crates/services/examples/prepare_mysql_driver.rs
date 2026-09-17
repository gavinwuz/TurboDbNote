use std::{path::PathBuf, sync::atomic::AtomicBool};
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(
        args.first()
            .ok_or("usage: prepare_mysql_driver ROOT [ZIP] [SOURCE]")?,
    );
    let zip = args.get(1).map(PathBuf::from);
    let source = args.get(2).map(PathBuf::from);
    let dll = turbodbn_services::mysql_drivers::install(
        &root,
        zip.as_deref(),
        &AtomicBool::new(false),
        |s| println!("{s}"),
    )?;
    turbodbn_services::mysql_drivers::prepare_source(&root, source.as_deref())?;
    println!("Prepared {}", dll.display());
    Ok(())
}
