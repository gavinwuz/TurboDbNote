use super::*;
use crate::database::clickhouse_fixture::{Fixture, NATIVE};
fn endpoint(f: &Fixture) -> Endpoint {
    Endpoint {
        profile: f.profile.clone(),
        password: String::new(),
    }
}
fn job(f: &Fixture, mode: Mode, file: PathBuf, schema: bool, options: Options) -> Job {
    Job {
        mode,
        source: endpoint(f),
        target: (mode != Mode::Export).then(|| endpoint(f)),
        database: "fixture".into(),
        file,
        contents: Contents { schema, data: true },
        options,
    }
}
#[test]
fn clickhouse_ddl_relocation_keeps_literals_and_column_aliases() {
    let ddl = "CREATE TABLE `fixture`.`items` (id UInt64, v String DEFAULT 'fixture.items') ENGINE=MergeTree ORDER BY id";
    let output = local_ddl(ddl, "fixture").unwrap();
    assert!(output.starts_with("CREATE TABLE IF NOT EXISTS `items`"));
    assert!(output.contains("'fixture.items'"));
    let view = "CREATE VIEW fixture.v AS SELECT fixture.id FROM fixture.items AS fixture";
    let output = local_ddl(view, "fixture").unwrap();
    assert!(output.contains("SELECT fixture.id FROM items AS fixture"));
    assert_eq!(lit("a\\b'\nc"), "'a\\\\b\\'\\nc'");
}
#[test]
fn clickhouse_dependencies_are_sorted_and_cycles_rejected() {
    let item = |name: &str, ddl: &str| Item {
        name: name.into(),
        engine: "View".into(),
        ddl: ddl.into(),
    };
    let items = vec![
        item("v2", "CREATE VIEW v2 AS SELECT * FROM v1"),
        item("v1", "CREATE VIEW v1 AS SELECT * FROM t"),
        item(
            "t",
            "CREATE TABLE t(id UInt64) ENGINE=MergeTree ORDER BY id",
        ),
    ];
    assert_eq!(
        restore_order(&items)
            .unwrap()
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        vec!["t", "v1", "v2"]
    );
    let items = vec![
        item("a", "CREATE VIEW a AS SELECT * FROM b"),
        item("b", "CREATE VIEW b AS SELECT * FROM a"),
    ];
    assert!(restore_order(&items).is_err());
}
#[test]
fn clickhouse_native_bundle_roundtrip_and_single_table_files() {
    let f = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let c = Control::default();
    let (tx, _) = async_channel::bounded(1000);
    let file = directory.path().join("database.zip");
    run(
        job(&f, Mode::Export, file.clone(), true, Options::default()),
        &c,
        &tx,
    )
    .unwrap();
    let mut archive = zip::ZipArchive::new(File::open(&file).unwrap()).unwrap();
    let manifest: Manifest =
        serde_json::from_reader(archive.by_name("manifest.json").unwrap()).unwrap();
    assert_eq!(manifest.objects.len(), 1);
    assert!(
        manifest.objects[0]
            .ddl
            .as_ref()
            .unwrap()
            .contains("CREATE TABLE IF NOT EXISTS items")
    );
    drop(archive);
    run(
        job(&f, Mode::ImportFile, file, true, Options::default()),
        &c,
        &tx,
    )
    .unwrap();
    assert!(
        f.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.sql.starts_with("INSERT INTO") && r.payload == NATIVE)
    );
    let file = directory.path().join("items.native");
    let options = Options {
        tables: Some(vec!["items".into()]),
        ..Default::default()
    };
    run(
        job(&f, Mode::Export, file.clone(), false, options.clone()),
        &c,
        &tx,
    )
    .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), NATIVE);
    run(job(&f, Mode::ImportFile, file, false, options), &c, &tx).unwrap();
}
#[test]
fn clickhouse_sql_dump_preserves_strings_and_reimports_without_rewriting() {
    let f = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let c = Control::default();
    let (tx, _) = async_channel::bounded(1000);
    let file = directory.path().join("database.sql");
    run(
        job(&f, Mode::Export, file.clone(), true, Options::default()),
        &c,
        &tx,
    )
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("VALUES (1,'a;\\\'b')"));
    let result = run(
        job(&f, Mode::ImportFile, file, true, Options::default()),
        &c,
        &tx,
    );
    assert!(
        result.is_ok(),
        "import failed: {:?}; request count {}",
        result,
        f.seen.lock().unwrap().len()
    );
    assert!(f.seen.lock().unwrap().iter().any(|r| {
        r.sql.trim_start().starts_with("INSERT INTO `items`") && r.sql.contains("a;\\\'b")
    }));
}
#[test]
fn clickhouse_import_validates_entire_file_before_writing() {
    let f = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let c = Control::default();
    let (tx, _) = async_channel::bounded(1000);
    let file = directory.path().join("bad.sql");
    std::fs::write(
        &file,
        "INSERT INTO items VALUES (1,'a'); ALTER TABLE other.items UPDATE label='x' WHERE id=1;",
    )
    .unwrap();
    assert!(
        run(
            job(&f, Mode::ImportFile, file, true, Options::default()),
            &c,
            &tx
        )
        .is_err()
    );
    assert!(
        !f.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.sql.starts_with("INSERT"))
    );
}

#[test]
fn clickhouse_bundle_rejects_corrupt_data_before_any_write() {
    let f = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let c = Control::default();
    let (tx, _) = async_channel::bounded(1000);
    let file = directory.path().join("bad.zip");
    let mut zip = zip::ZipWriter::new(File::create(&file).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("data/0.native", opts).unwrap();
    zip.write_all(NATIVE).unwrap();
    zip.start_file("manifest.json", opts).unwrap();
    let manifest = Manifest {
        version: 1,
        objects: vec![BundleObject {
            name: "items".into(),
            ddl: Some(
                "CREATE TABLE items(id UInt64,label String) ENGINE=MergeTree ORDER BY id".into(),
            ),
            data: Some("data/0.native".into()),
            columns: Some("`id`,`label`".into()),
            sha256: Some("0".repeat(64)),
        }],
    };
    serde_json::to_writer(&mut zip, &manifest).unwrap();
    zip.finish().unwrap();
    assert!(
        run(
            job(&f, Mode::ImportFile, file, true, Options::default()),
            &c,
            &tx
        )
        .is_err()
    );
    assert!(
        !f.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.sql.starts_with("CREATE") || r.sql.starts_with("INSERT"))
    );
}

#[test]
fn clickhouse_cancel_and_existing_destination_do_not_publish_or_overwrite() {
    let f = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let c = Control::default();
    let (tx, _) = async_channel::bounded(1000);
    let file = directory.path().join("existing.sql");
    std::fs::write(&file, b"keep").unwrap();
    assert!(
        run(
            job(&f, Mode::Export, file.clone(), true, Options::default()),
            &c,
            &tx
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"keep");
    c.cancel();
    let file = directory.path().join("cancelled.zip");
    assert!(
        run(
            job(&f, Mode::Export, file.clone(), true, Options::default()),
            &c,
            &tx
        )
        .is_err()
    );
    assert!(!file.exists());
}
