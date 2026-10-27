//! Read-only dump preflight; emits aggregate counts, never SQL or credentials.
fn main() {
    let result = (|| {
        let path = std::env::args().nth(1).ok_or("restore.limit")?;
        let source = std::fs::read_to_string(path).map_err(|_| "restore.limit")?;
        let options = turbodbn_services::database::mysql_restore::Options {
            database: "local_restore_preview".into(),
            data: false,
            triggers: true,
            current_definer: true,
        };
        let p = turbodbn_services::database::mysql_restore::adapt(&source, &options)?;
        println!(
            "Tables: {}; duplicates skipped: {}; DROP removed: {}; data skipped: {}; triggers skipped: {}; definers changed: {}; generated columns: {}",
            p.tables,
            p.duplicates,
            p.removed_drops,
            p.skipped_data,
            p.skipped_triggers,
            p.changed_definers,
            p.generated_columns
        );
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        eprintln!("Preflight category: {error}");
        std::process::exit(1);
    }
}
