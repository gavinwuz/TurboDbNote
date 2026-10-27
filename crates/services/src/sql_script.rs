//! Immutable script attachments and bounded indexes. SQL text never enters application journals.
use crate::{DatabaseKind, database::notebook_query};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use turbodbn_core::notebook::{SqlDialect, SqlScript};

pub const MAX_SCRIPT_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_STATEMENTS: u64 = 1_000_000;
pub const MAX_SQL_BYTES: usize = notebook_query::SCRIPT_MAX_BYTES;
pub const PAGE_SIZE: u64 = 6;
const HEADER: u64 = 80;
const RECORD: u64 = 32;
const MAGIC: &[u8; 8] = b"TDBSQL04";
pub fn kind(dialect: SqlDialect) -> DatabaseKind {
    match dialect {
        SqlDialect::MySql => DatabaseKind::MySql,
        SqlDialect::PostgreSql => DatabaseKind::PostgreSql,
        SqlDialect::Sqlite => DatabaseKind::Sqlite,
        SqlDialect::ClickHouse => DatabaseKind::ClickHouse,
    }
}
pub fn dialect(kind: DatabaseKind) -> Option<SqlDialect> {
    match kind {
        DatabaseKind::MySql => Some(SqlDialect::MySql),
        DatabaseKind::PostgreSql => Some(SqlDialect::PostgreSql),
        DatabaseKind::Sqlite => Some(SqlDialect::Sqlite),
        DatabaseKind::ClickHouse => Some(SqlDialect::ClickHouse),
        _ => None,
    }
}
fn code(dialect: SqlDialect) -> u8 {
    match dialect {
        SqlDialect::MySql => 0,
        SqlDialect::PostgreSql => 1,
        SqlDialect::Sqlite => 2,
        SqlDialect::ClickHouse => 3,
    }
}
fn disk(_: impl std::fmt::Display) -> String {
    "script.error.disk".into()
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("script.cancelled".into())
    } else {
        Ok(())
    }
}
fn hash_file(file: &mut File, cancel: &AtomicBool) -> Result<String, String> {
    file.rewind().map_err(disk)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        check(cancel)?;
        let n = file.read(&mut buffer).map_err(disk)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    file.rewind().map_err(disk)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn open_read(path: &Path) -> Result<File, String> {
    if fs::symlink_metadata(path)
        .map_err(disk)?
        .file_type()
        .is_symlink()
    {
        return Err("script.error.resource".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    options.open(path).map_err(disk)
}
fn folder(note: &Path) -> Result<PathBuf, String> {
    let parent = note.parent().ok_or("script.error.resource")?;
    let root = parent.join(".turbodbnote-scripts");
    if root.exists()
        && fs::symlink_metadata(&root)
            .map_err(disk)?
            .file_type()
            .is_symlink()
    {
        return Err("script.error.resource".into());
    }
    Ok(root)
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub number: u64,
    pub line: u64,
    pub bytes: u64,
    pub supported: bool,
    pub write: bool,
    pub preview: String,
    pub has_comments: bool,
    pub reason: String,
}
/// Display-only segmentation; execution retains the immutable original source.
pub struct StatementPreview {
    pub source: String,
    pub sql: String,
    pub comments: String,
    pub source_line: u64,
    pub sql_line: u64,
    pub end_line: u64,
}
pub fn leading_code_offset(sql: &str, dialect: SqlDialect) -> usize {
    let b = sql.as_bytes();
    let mut i = 0;
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i..i + 2) == Some(b"--")
            && (dialect != SqlDialect::MySql
                || b.get(i + 2)
                    .is_none_or(|b| b.is_ascii_whitespace() || b.is_ascii_control()))
            || dialect == SqlDialect::MySql && b.get(i) == Some(&b'#')
        {
            while b.get(i).is_some_and(|b| *b != b'\n') {
                i += 1;
            }
        } else if b.get(i..i + 2) == Some(b"/*")
            && b.get(i + 2) != Some(&b'!')
            && b.get(i..i + 4) != Some(b"/*M!")
        {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if dialect == SqlDialect::PostgreSql && b.get(i..i + 2) == Some(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b.get(i..i + 2) == Some(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else {
            return i;
        }
    }
}
pub fn statement_preview(source: String, sql_line: u64, dialect: SqlDialect) -> StatementPreview {
    let start = leading_code_offset(&source, dialect);
    let prefix = &source[..start];
    let source_line = sql_line
        .saturating_sub(prefix.bytes().filter(|b| *b == b'\n').count() as u64)
        .max(1);
    let end_line = source_line + source.bytes().filter(|b| *b == b'\n').count() as u64;
    StatementPreview {
        sql: source[start..].to_owned(),
        comments: prefix.trim().to_owned(),
        source_line,
        sql_line,
        end_line,
        source,
    }
}
#[derive(Clone, Debug)]
pub struct PermissionBlock {
    pub number: u64,
    pub line: u64,
    pub command: String,
}
#[derive(Clone, Default)]
pub struct AuthorizationSummary {
    pub reads: u64,
    pub writes: u64,
    pub ddl: u64,
    pub session: u64,
    pub locks: u64,
    pub transactions: u64,
    pub destructive: u64,
    pub relaxed_checks: u64,
    pub risky: u64,
}
impl AuthorizationSummary {
    pub fn requires_authorization(&self) -> bool {
        self.writes > 0
            || self.ddl > 0
            || self.session > 0
            || self.locks > 0
            || self.transactions > 0
            || self.destructive > 0
            || self.relaxed_checks > 0
            || self.risky > 0
    }
}
impl Reader {
    /// Original, hash-verified frozen attachment, bounded for combined Note export.
    pub fn export_source(&mut self, limit: u64) -> Result<String, String> {
        if self.script.bytes > limit {
            return Err("notes.export_limit".into());
        }
        self.source.seek(SeekFrom::Start(0)).map_err(disk)?;
        let mut bytes = Vec::new();
        (&mut self.source)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(disk)?;
        if bytes.len() as u64 > limit {
            return Err("notes.export_limit".into());
        }
        String::from_utf8(bytes).map_err(|_| "script.error.encoding".into())
    }

    pub fn database_kind(&self) -> DatabaseKind {
        kind(self.script.dialect)
    }
    pub fn authorization_summary(
        &mut self,
        start: u64,
        end: u64,
        legacy: bool,
        cancel: &AtomicBool,
    ) -> Result<AuthorizationSummary, String> {
        let mut result = AuthorizationSummary::default();
        let kind = kind(self.script.dialect);
        for number in start..end {
            check(cancel)?;
            let sql = self.statement(number)?;
            let key = notebook_query::script_command_required(kind, &sql)
                .ok_or("script.error.unsupported")?;
            match key {
                "script_session" | "script_variables" => result.session += 1,
                "script_lock" => result.locks += 1,
                "script_transaction" => result.transactions += 1,
                "create" | "alter" | "drop" | "truncate" | "script_drop_trigger" => result.ddl += 1,
                "insert" | "update" | "delete" => result.writes += 1,
                _ => result.reads += 1,
            }
            if matches!(key, "drop" | "truncate" | "delete" | "script_drop_trigger") {
                result.destructive += 1;
            }
            if key == "script_session" {
                let expanded = crate::database::workbench::script_context::expand(&sql, None)?;
                if crate::database::workbench::script_context::relaxes_checks(&expanded)? {
                    result.relaxed_checks += 1;
                }
            }
            if notebook_query::script_requires_confirmation(kind, &sql, legacy)? {
                result.risky += 1;
            }
        }
        Ok(result)
    }
    pub fn authorization_page(
        &mut self,
        start: u64,
        end: u64,
        page: u64,
        risk_only: bool,
        legacy: bool,
    ) -> Result<Vec<Entry>, String> {
        if !risk_only {
            return (start + page..(start + page + PAGE_SIZE).min(end))
                .map(|n| self.entry(n))
                .collect();
        }
        let mut found = 0;
        let mut rows = Vec::new();
        for number in start..end {
            let sql = self.statement(number)?;
            if notebook_query::script_requires_confirmation(
                kind(self.script.dialect),
                &sql,
                legacy,
            )? {
                if found >= page {
                    rows.push(self.entry(number)?);
                    if rows.len() == PAGE_SIZE as usize {
                        break;
                    }
                }
                found += 1;
            }
        }
        Ok(rows)
    }
}
pub fn permission_block(
    reader: &mut Reader,
    kind: DatabaseKind,
    number: u64,
    allowed: &[String],
) -> Result<Option<PermissionBlock>, String> {
    let sql = reader.statement(number)?;
    let command =
        notebook_query::script_command_required(kind, &sql).ok_or("script.error.unsupported")?;
    if allowed.iter().any(|key| key == command) {
        return Ok(None);
    }
    let label = notebook_query::COMMANDS
        .iter()
        .chain(notebook_query::SCRIPT_COMMANDS)
        .find(|(key, _, _)| *key == command)
        .map_or(command, |(_, label, _)| label);
    let (_, _, line, _) = reader.range(number)?;
    Ok(Some(PermissionBlock {
        number,
        line,
        command: label.into(),
    }))
}
pub struct Prepared {
    directory: tempfile::TempDir,
    pub script: SqlScript,
}
impl Prepared {
    pub fn reader(&self) -> Result<Reader, String> {
        Reader::from_root(self.directory.path(), &self.script)
    }
    pub fn page(&self) -> Result<Vec<Entry>, String> {
        Reader::from_root(self.directory.path(), &self.script)?.page(0)
    }
    pub fn publish(&self, note: &Path, cancel: &AtomicBool) -> Result<(), String> {
        let root = folder(note)?;
        fs::create_dir_all(&root).map_err(disk)?;
        for (hash, extension) in [
            (&self.script.source_hash, "sql"),
            (&self.script.index_hash, "idx"),
        ] {
            publish(
                &self.directory.path().join(format!("{hash}.{extension}")),
                &root.join(format!("{hash}.{extension}")),
                hash,
                cancel,
            )?;
        }
        Ok(())
    }
}
fn publish(
    source: &Path,
    target: &Path,
    expected: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut input = open_read(source)?;
    if target.exists() {
        let mut target = open_read(target)?;
        if hash_file(&mut target, cancel)? != expected {
            return Err("script.error.changed".into());
        }
        return Ok(());
    }
    let mut staged =
        tempfile::NamedTempFile::new_in(target.parent().ok_or("script.error.resource")?)
            .map_err(disk)?;
    let mut buffer = [0; 65536];
    let mut hash = Sha256::new();
    loop {
        check(cancel)?;
        let n = input.read(&mut buffer).map_err(disk)?;
        if n == 0 {
            break;
        }
        staged.write_all(&buffer[..n]).map_err(disk)?;
        hash.update(&buffer[..n]);
    }
    if format!("{:x}", hash.finalize()) != expected {
        return Err("script.error.changed".into());
    }
    staged.as_file().sync_all().map_err(disk)?;
    if let Err(error) = staged.persist_noclobber(target) {
        if error.error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(disk(error));
        }
        let mut file = open_read(target)?;
        if hash_file(&mut file, cancel)? != expected {
            return Err("script.error.changed".into());
        }
    }
    Ok(())
}
pub fn copy_resources(from: &Path, to: &Path, note: &turbodbn_core::Note) -> Result<(), String> {
    if from.parent() == to.parent() {
        return Ok(());
    }
    let cancel = AtomicBool::new(false);
    for cell in &note.cells {
        if let turbodbn_core::CellContent::SqlScript { script, .. } = &cell.content {
            if !script.valid() {
                return Err("script.error.resource".into());
            }
            let source = folder(from)?;
            let target = folder(to)?;
            fs::create_dir_all(&target).map_err(disk)?;
            for (hash, extension) in [(&script.source_hash, "sql"), (&script.index_hash, "idx")] {
                publish(
                    &source.join(format!("{hash}.{extension}")),
                    &target.join(format!("{hash}.{extension}")),
                    hash,
                    &cancel,
                )?;
            }
        }
    }
    Ok(())
}

/// Prepare an editor buffer through the same immutable attachment path as file imports.
pub fn prepare_text(
    source: &str,
    dialect: SqlDialect,
    cancel: &AtomicBool,
) -> Result<Prepared, String> {
    let mut temp = tempfile::Builder::new()
        .suffix(".sql")
        .tempfile()
        .map_err(disk)?;
    temp.write_all(source.as_bytes()).map_err(disk)?;
    temp.flush().map_err(disk)?;
    // Close the writable handle before open_read applies its Windows sharing contract.
    let path = temp.into_temp_path();
    let mut prepared = prepare(&path, dialect, cancel, |_, _| {})?;
    prepared.script.name = "Note SQL.sql".into();
    Ok(prepared)
}
pub fn prepare(
    path: &Path,
    dialect: SqlDialect,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sql"))
    {
        return Err("script.error.extension".into());
    }
    let mut input = open_read(path)?;
    let original = input.metadata().map_err(disk)?;
    if original.len() > MAX_SCRIPT_BYTES {
        return Err("script.error.file_limit".into());
    }
    let directory = tempfile::tempdir().map_err(disk)?;
    let source_path = directory.path().join("source.sql");
    let mut source = File::create(&source_path).map_err(disk)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut carry = Vec::new();
    let mut bytes = 0;
    loop {
        check(cancel)?;
        let n = input.read(&mut buffer).map_err(disk)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > MAX_SCRIPT_BYTES {
            return Err("script.error.file_limit".into());
        }
        if buffer[..n].contains(&0) {
            return Err("script.error.encoding".into());
        }
        carry.extend_from_slice(&buffer[..n]);
        match std::str::from_utf8(&carry) {
            Ok(_) => carry.clear(),
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                carry.drain(..valid);
            }
            Err(_) => return Err("script.error.encoding".into()),
        }
        source.write_all(&buffer[..n]).map_err(disk)?;
        hash.update(&buffer[..n]);
        progress(bytes, 0);
    }
    if !carry.is_empty() {
        return Err("script.error.encoding".into());
    }
    let after = input.metadata().map_err(disk)?;
    if original.len() != after.len() || original.modified().ok() != after.modified().ok() {
        return Err("script.error.changed".into());
    }
    source.sync_all().map_err(disk)?;
    drop(source);
    drop(input);
    let source_hash = format!("{:x}", hash.finalize());
    let index_path = directory.path().join("index.idx");
    let mut index = BufWriter::new(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&index_path)
            .map_err(disk)?,
    );
    index.write_all(&[0; HEADER as usize]).map_err(disk)?;
    let mut reader = BufReader::new(File::open(&source_path).map_err(disk)?);
    let bom = reader
        .fill_buf()
        .map_err(disk)?
        .starts_with(&[0xef, 0xbb, 0xbf]);
    if bom {
        reader.consume(3);
    }
    let mut scan = Scanner::new(dialect, if bom { 3 } else { 0 });
    for byte in reader.bytes() {
        check_byte(&mut scan, byte.map_err(disk)?, &mut index, cancel)?;
        if scan.offset.is_multiple_of(65536) {
            progress(bytes, scan.count);
        }
    }
    if !scan.finished() {
        return Err("script.error.unterminated".into());
    }
    scan.emit(&mut index)?;
    if scan.count == 0 {
        return Err("script.error.empty".into());
    }
    index.flush().map_err(disk)?;
    let mut index = index.into_inner().map_err(disk)?;
    index.rewind().map_err(disk)?;
    index.write_all(MAGIC).map_err(disk)?;
    index
        .write_all(
            source_hash
                .as_bytes()
                .get(..32)
                .ok_or("script.error.resource")?,
        )
        .map_err(disk)?;
    index
        .write_all(&[code(dialect), 0, 0, 0, 0, 0, 0, 0])
        .map_err(disk)?;
    for value in [scan.count, scan.unsupported, bytes, scan.writes] {
        index.write_all(&value.to_le_bytes()).map_err(disk)?;
    }
    index.sync_all().map_err(disk)?;
    let index_hash = hash_file(&mut index, cancel)?;
    drop(index);
    fs::rename(
        source_path,
        directory.path().join(format!("{source_hash}.sql")),
    )
    .map_err(disk)?;
    fs::rename(
        index_path,
        directory.path().join(format!("{index_hash}.idx")),
    )
    .map_err(disk)?;
    let name = path
        .file_name()
        .ok_or("script.error.extension")?
        .to_string_lossy()
        .into_owned();
    let script = SqlScript {
        name,
        source_hash,
        index_hash,
        bytes,
        statements: scan.count,
        unsupported: scan.unsupported,
        writes: scan.writes,
        dialect,
        restore: None,
    };
    if !script.valid() {
        return Err("script.error.resource".into());
    }
    Ok(Prepared { directory, script })
}
/// Validate a bounded export without the editor's 1 MiB total-buffer restriction.
pub fn validate_export_source(source: &str, dialect: SqlDialect) -> Result<(), String> {
    if source.contains('\0') {
        return Err("notes.sql_range_invalid".into());
    }
    let mut scanner = Scanner::new(dialect, 0);
    let mut sink = std::io::sink();
    for byte in source.bytes() {
        scanner.push(byte, &mut sink)?;
    }
    if !scanner.finished() {
        return Err("script.error.unterminated".into());
    }
    scanner.emit(&mut sink)
}
/// Bounded editor segmentation reuses the script lexer and its byte-exact records.
pub fn editor_ranges(
    source: &str,
    dialect: SqlDialect,
) -> Result<Vec<std::ops::Range<usize>>, String> {
    if source.len() > MAX_SQL_BYTES || source.contains('\0') {
        return Err("notes.sql_range_invalid".into());
    }
    let mut scanner = Scanner::new(dialect, 0);
    let mut records = Vec::new();
    for byte in source.bytes() {
        scanner.push(byte, &mut records)?;
    }
    if !scanner.finished() {
        return Err("script.error.unterminated".into());
    }
    scanner.emit(&mut records)?;
    let mut ranges = records
        .chunks_exact(RECORD as usize)
        .map(|record| {
            let start = u64::from_le_bytes(record[0..8].try_into().unwrap()) as usize;
            let length = u64::from_le_bytes(record[8..16].try_into().unwrap()) as usize;
            start..start + length
        })
        .collect::<Vec<_>>();
    if let Some(last) = ranges.last_mut() {
        last.end = source.len();
    }
    Ok(ranges)
}
/// SQL query units execute a complete selection, or all statements when no selection exists.
/// Execution plans keep the current-statement behavior when nothing is selected.
pub fn editor_query_target(
    source: &str,
    dialect: SqlDialect,
    selection: std::ops::Range<usize>,
    cursor: usize,
    explain: bool,
) -> Result<Vec<std::ops::Range<usize>>, String> {
    let selection = if !explain && selection.is_empty() {
        0..source.len()
    } else {
        selection
    };
    editor_target(source, dialect, selection, cursor).map(|(ranges, _)| ranges)
}
/// A selection must contain complete statement code. Leading/trailing whitespace may be omitted.
pub fn editor_target(
    source: &str,
    dialect: SqlDialect,
    selection: std::ops::Range<usize>,
    cursor: usize,
) -> Result<(Vec<std::ops::Range<usize>>, usize), String> {
    let ranges = editor_ranges(source, dialect)?;
    if ranges.is_empty() {
        return Err("script.error.empty".into());
    }
    if selection.start > selection.end
        || selection.end > source.len()
        || !source.is_char_boundary(selection.start)
        || !source.is_char_boundary(selection.end)
    {
        return Err("notes.sql_range_invalid".into());
    }
    let current = ranges
        .iter()
        .position(|range| cursor >= range.start && cursor < range.end)
        .unwrap_or(ranges.len() - 1);
    if selection.is_empty() {
        return Ok((vec![ranges[current].clone()], current));
    }
    let chosen = ranges
        .iter()
        .filter(|range| {
            range.start < selection.end
                && range.end > selection.start
                && !source[range.start.max(selection.start)..range.end.min(selection.end)]
                    .trim()
                    .is_empty()
        })
        .cloned()
        .collect::<Vec<_>>();
    let no_code = |text: &str| {
        let text = text.trim().trim_start_matches(';');
        leading_code_offset(text, dialect) >= text.len()
    };
    if chosen.iter().any(|range| {
        !no_code(&source[range.start..range.start.max(selection.start)])
            || !no_code(&source[range.end.min(selection.end)..range.end])
    }) || chosen.is_empty()
    {
        return Err("notes.sql_selection_incomplete".into());
    }
    Ok((chosen, current))
}

fn check_byte(
    scan: &mut Scanner,
    byte: u8,
    index: &mut impl Write,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if scan.offset.is_multiple_of(65536) {
        check(cancel)?;
    }
    scan.push(byte, index)
}
enum Mode {
    Code,
    Quote { q: u8, end: bool, escape: bool },
    Line,
    Dash,
    Block { depth: u32, prev: u8 },
    DollarOpen(Vec<u8>),
    Dollar { tag: Vec<u8>, matched: usize },
}
struct Scanner {
    dialect: SqlDialect,
    mode: Mode,
    offset: u64,
    start: u64,
    line: u64,
    first_line: u64,
    has_code: bool,
    pending: Option<(u8, bool, u64)>,
    body: Vec<u8>,
    unsafe_quote: bool,
    count: u64,
    unsupported: u64,
    writes: u64,
}
impl Scanner {
    fn new(dialect: SqlDialect, start: u64) -> Self {
        Self {
            dialect,
            mode: Mode::Code,
            offset: start,
            start,
            line: 1,
            first_line: 1,
            has_code: false,
            pending: None,
            body: Vec::new(),
            unsafe_quote: false,
            count: 0,
            unsupported: 0,
            writes: 0,
        }
    }
    fn finished(&self) -> bool {
        matches!(
            self.mode,
            Mode::Code
                | Mode::Line
                | Mode::Dash
                | Mode::DollarOpen(_)
                | Mode::Quote { end: true, .. }
        )
    }
    fn push(&mut self, b: u8, out: &mut impl Write) -> Result<(), String> {
        self.offset += 1;
        if self.body.len() <= MAX_SQL_BYTES {
            self.body.push(b);
        }
        let mut code = false;
        match &mut self.mode {
            Mode::Code => code = true,
            Mode::Line => {
                if b == b'\n' {
                    self.mode = Mode::Code;
                    self.pending = None;
                }
            }
            Mode::Dash => {
                if b.is_ascii_whitespace() {
                    self.mode = if b == b'\n' { Mode::Code } else { Mode::Line };
                } else {
                    self.mode = Mode::Code;
                    self.has_code = true;
                    code = true;
                }
            }
            Mode::Block { depth, prev } => {
                if self.dialect == SqlDialect::MySql && *depth == 1 && *prev == 0 && b == b'!' {
                    self.has_code = true;
                    self.first_line = self.line;
                }
                if *prev == b'/' && b == b'*' {
                    if self.dialect == SqlDialect::PostgreSql {
                        *depth = depth.checked_add(1).ok_or("script.error.unterminated")?;
                    } else {
                        self.unsafe_quote = true;
                    }
                    *prev = 0;
                } else if *prev == b'*' && b == b'/' {
                    *depth -= 1;
                    if *depth == 0 {
                        self.mode = Mode::Code;
                        self.pending = None;
                    } else {
                        *prev = 0;
                    }
                } else {
                    *prev = b;
                }
            }
            Mode::Quote { q, end, escape } => {
                if *escape {
                    *escape = false;
                } else if *end {
                    if b == *q {
                        *end = false;
                    } else {
                        self.mode = Mode::Code;
                        self.pending = None;
                        code = true;
                    }
                } else if b == *q {
                    *end = true;
                } else if b == b'\\' && self.dialect != SqlDialect::Sqlite {
                    *escape = true;
                    self.unsafe_quote |=
                        !matches!(self.dialect, SqlDialect::MySql | SqlDialect::ClickHouse)
                            || (self.dialect == SqlDialect::MySql && *q == b'`');
                }
            }
            Mode::DollarOpen(tag) => {
                if b == b'$' {
                    tag.push(b);
                    self.mode = Mode::Dollar {
                        tag: tag.clone(),
                        matched: 0,
                    };
                } else if (b.is_ascii_alphanumeric() || b == b'_') && tag.len() < 128 {
                    tag.push(b);
                } else {
                    self.mode = Mode::Code;
                    self.pending = None;
                    code = true;
                }
            }
            Mode::Dollar { tag, matched } => {
                if b == tag[*matched] {
                    *matched += 1;
                    if *matched == tag.len() {
                        self.mode = Mode::Code;
                        self.pending = None;
                    }
                } else {
                    *matched = usize::from(b == b'$');
                }
            }
        }
        if code {
            if let Some((previous, had_code, first_line)) = self.pending.take()
                && (previous == b'-' && b == b'-' || previous == b'/' && b == b'*')
            {
                self.has_code = had_code;
                self.first_line = first_line;
                self.mode = if b == b'-' {
                    if self.dialect == SqlDialect::MySql {
                        Mode::Dash
                    } else {
                        Mode::Line
                    }
                } else {
                    Mode::Block { depth: 1, prev: 0 }
                };
                if b == b'\n' {
                    self.line += 1;
                }
                return Ok(());
            }
            let had_code = self.has_code;
            let first_line = self.first_line;
            if b == b'#' && self.dialect == SqlDialect::MySql {
                self.mode = Mode::Line;
                return Ok(());
            }
            if !b.is_ascii_whitespace() && b != b';' {
                if !self.has_code {
                    self.first_line = self.line;
                }
                self.has_code = true;
            }
            match b {
                b'-' | b'/' => self.pending = Some((b, had_code, first_line)),
                b'\'' | b'"' | b'`' => {
                    self.mode = Mode::Quote {
                        q: b,
                        end: false,
                        escape: false,
                    }
                }
                b'[' if self.dialect == SqlDialect::Sqlite => {
                    self.mode = Mode::Quote {
                        q: b']',
                        end: false,
                        escape: false,
                    };
                }
                b'$' if self.dialect == SqlDialect::PostgreSql
                    && !self
                        .body
                        .get(self.body.len().saturating_sub(2))
                        .is_some_and(|previous| {
                            previous.is_ascii_alphanumeric()
                                || *previous == b'_'
                                || *previous == b'$'
                                || *previous >= 128
                        }) =>
                {
                    self.mode = Mode::DollarOpen(vec![b'$'])
                }
                b';' => self.emit(out)?,
                _ => {}
            }
        }
        if b == b'\n' {
            self.line += 1;
        }
        Ok(())
    }
    fn emit(&mut self, out: &mut impl Write) -> Result<(), String> {
        if self.has_code {
            if self.count == MAX_STATEMENTS {
                return Err("script.error.statement_limit".into());
            }
            let classification = if !self.unsafe_quote && self.body.len() <= MAX_SQL_BYTES {
                std::str::from_utf8(&self.body).ok().and_then(|sql| {
                    notebook_query::validate_script_statement(kind(self.dialect), sql).ok()
                })
            } else {
                None
            };
            let supported = classification.is_some();
            let write = classification == Some(false);
            for value in [self.start, self.offset - self.start, self.first_line] {
                out.write_all(&value.to_le_bytes()).map_err(disk)?;
            }
            out.write_all(&[
                if write { 2 } else { u8::from(supported) },
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ])
            .map_err(disk)?;
            self.count += 1;
            self.unsupported += u64::from(!supported);
            self.writes += u64::from(write);
        }
        self.body.clear();
        self.start = self.offset;
        self.has_code = false;
        self.pending = None;
        self.unsafe_quote = false;
        Ok(())
    }
}

pub struct Reader {
    source: File,
    index: File,
    script: SqlScript,
    _frozen: Option<tempfile::TempDir>,
}
impl Reader {
    fn from_root(root: &Path, script: &SqlScript) -> Result<Self, String> {
        if !script.valid() {
            return Err("script.error.resource".into());
        }
        let source = open_read(&root.join(format!("{}.sql", script.source_hash)))?;
        let mut index = open_read(&root.join(format!("{}.idx", script.index_hash)))?;
        if source.metadata().map_err(disk)?.len() != script.bytes
            || index.metadata().map_err(disk)?.len() != HEADER + script.statements * RECORD
        {
            return Err("script.error.changed".into());
        }
        let mut header = [0; HEADER as usize];
        index.read_exact(&mut header).map_err(disk)?;
        if (&header[..8] != MAGIC
            && &header[..8] != b"TDBSQL01"
            && &header[..8] != b"TDBSQL02"
            && &header[..8] != b"TDBSQL03")
            || header[8..40] != script.source_hash.as_bytes()[..32]
            || header[40] != code(script.dialect)
            || u64::from_le_bytes(header[48..56].try_into().unwrap()) != script.statements
            || u64::from_le_bytes(header[56..64].try_into().unwrap()) != script.unsupported
            || u64::from_le_bytes(header[64..72].try_into().unwrap()) != script.bytes
            || u64::from_le_bytes(header[72..80].try_into().unwrap()) != script.writes
        {
            return Err("script.error.changed".into());
        }
        Ok(Self {
            source,
            index,
            script: script.clone(),
            _frozen: None,
        })
    }
    fn range(&mut self, number: u64) -> Result<(u64, u64, u64, u8), String> {
        if number >= self.script.statements {
            return Err("script.error.resource".into());
        }
        self.index
            .seek(SeekFrom::Start(HEADER + number * RECORD))
            .map_err(disk)?;
        let mut raw = [0; RECORD as usize];
        self.index.read_exact(&mut raw).map_err(disk)?;
        let start = u64::from_le_bytes(raw[..8].try_into().unwrap());
        let len = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        let line = u64::from_le_bytes(raw[16..24].try_into().unwrap());
        if start
            .checked_add(len)
            .is_none_or(|end| end > self.script.bytes)
            || raw[24] > 2
        {
            return Err("script.error.resource".into());
        }
        Ok((start, len, line, raw[24]))
    }
    pub fn statement_line(&mut self, number: u64) -> Result<u64, String> {
        self.range(number).map(|(_, _, line, _)| line)
    }
    pub fn statement(&mut self, number: u64) -> Result<String, String> {
        let (start, len, _, supported) = self.range(number)?;
        if supported == 0 {
            return Err("script.error.unsupported".into());
        }
        self.read_statement(start, len)
    }
    pub fn raw_statement(&mut self, number: u64) -> Result<String, String> {
        let (start, len, _, _) = self.range(number)?;
        self.read_statement(start, len)
    }
    fn read_statement(&mut self, start: u64, len: u64) -> Result<String, String> {
        if len > MAX_SQL_BYTES as u64 {
            return Err("script.error.statement_size".into());
        }
        self.source.seek(SeekFrom::Start(start)).map_err(disk)?;
        let mut bytes = vec![0; len as usize];
        self.source.read_exact(&mut bytes).map_err(disk)?;
        String::from_utf8(bytes).map_err(|_| "script.error.encoding".into())
    }
    fn entry(&mut self, number: u64) -> Result<Entry, String> {
        let (offset, len, line, supported) = self.range(number)?;
        self.source.seek(SeekFrom::Start(offset)).map_err(disk)?;
        let mut bytes = vec![0; len.min(512) as usize];
        self.source.read_exact(&mut bytes).map_err(disk)?;
        let valid =
            std::str::from_utf8(&bytes).map_or_else(|error| error.valid_up_to(), |_| bytes.len());
        let source = if len <= MAX_SQL_BYTES as u64 {
            self.read_statement(offset, len)?
        } else {
            std::str::from_utf8(&bytes[..valid])
                .map_err(|_| "script.error.encoding")?
                .to_owned()
        };
        let start = leading_code_offset(&source, self.script.dialect);
        let has_comments = !source[..start].trim().is_empty();
        let prefix = source[start..].chars().take(512).collect::<String>();
        let preview = prefix.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview = preview.chars().take(128).collect::<String>();
        let reason = if supported == 0 {
            self.read_statement(offset, len)
                .and_then(|sql| {
                    notebook_query::validate_script_statement(kind(self.script.dialect), &sql)
                })
                .err()
                .unwrap_or_else(|| "script.error.unsupported".into())
        } else {
            String::new()
        };
        Ok(Entry {
            number,
            line,
            bytes: len,
            supported: supported > 0,
            write: supported == 2,
            preview,
            has_comments,
            reason,
        })
    }
    pub fn page(&mut self, start: u64) -> Result<Vec<Entry>, String> {
        (start..start.saturating_add(PAGE_SIZE).min(self.script.statements))
            .map(|number| self.entry(number))
            .collect()
    }
}
pub fn page(note: &Path, script: &SqlScript, start: u64) -> Result<Vec<Entry>, String> {
    Reader::from_root(&folder(note)?, script)?.page(start)
}
/// Scan only the disk index, retaining one page of original statement numbers.
pub fn issue_page(note: &Path, script: &SqlScript, start: u64) -> Result<Vec<Entry>, String> {
    let mut reader = Reader::from_root(&folder(note)?, script)?;
    reader.index.seek(SeekFrom::Start(HEADER)).map_err(disk)?;
    let mut index = BufReader::new(reader.index.try_clone().map_err(disk)?);
    let mut numbers = Vec::new();
    let mut found = 0;
    for number in 0..script.statements {
        let mut record = [0; RECORD as usize];
        index.read_exact(&mut record).map_err(disk)?;
        if record[24] == 0 {
            if found >= start {
                numbers.push(number);
                if numbers.len() == PAGE_SIZE as usize {
                    break;
                }
            }
            found += 1;
        }
    }
    drop(index);
    let mut rows = Vec::with_capacity(numbers.len());
    for number in numbers {
        rows.push(reader.entry(number)?);
    }
    Ok(rows)
}
pub fn preview(note: &Path, script: &SqlScript, number: u64) -> Result<String, String> {
    let mut reader = Reader::from_root(&folder(note)?, script)?;
    let (start, len, _, _) = reader.range(number)?;
    reader.read_statement(start, len)
}
/// Upgrade only the derived immutable index; preserve source hash and document identity.
pub fn refresh(note: &Path, script: &SqlScript, cancel: &AtomicBool) -> Result<SqlScript, String> {
    let root = folder(note)?;
    let mut index = open_read(&root.join(format!("{}.idx", script.index_hash)))?;
    let mut magic = [0; 8];
    index.read_exact(&mut magic).map_err(disk)?;
    if &magic == MAGIC {
        return Ok(script.clone());
    }
    if &magic != b"TDBSQL01" && &magic != b"TDBSQL02" && &magic != b"TDBSQL03" {
        return Err("script.error.resource".into());
    }
    drop(index);
    reparse(note, script, script.dialect, cancel)
}
/// Rebuild only derived classification; original attachment bytes and name stay unchanged.
pub fn reparse(
    note: &Path,
    script: &SqlScript,
    dialect: SqlDialect,
    cancel: &AtomicBool,
) -> Result<SqlScript, String> {
    let root = folder(note)?;
    let source = root.join(format!("{}.sql", script.source_hash));
    let mut prepared = prepare(&source, dialect, cancel, |_, _| {})?;
    if prepared.script.source_hash != script.source_hash {
        return Err("script.error.changed".into());
    }
    prepared.script.name = script.name.clone();
    prepared.script.restore = script.restore.clone();
    prepared.publish(note, cancel)?;
    Ok(prepared.script)
}
pub fn freeze(note: &Path, script: &SqlScript, cancel: &AtomicBool) -> Result<Reader, String> {
    let directory = tempfile::tempdir().map_err(disk)?;
    let root = folder(note)?;
    for (hash, extension) in [(&script.source_hash, "sql"), (&script.index_hash, "idx")] {
        publish(
            &root.join(format!("{hash}.{extension}")),
            &directory.path().join(format!("{hash}.{extension}")),
            hash,
            cancel,
        )?;
    }
    let mut reader = Reader::from_root(directory.path(), script)?;
    reader._frozen = Some(directory);
    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_read_only_batches_bypass_authorization() {
        for dialect in [
            SqlDialect::MySql,
            SqlDialect::PostgreSql,
            SqlDialect::Sqlite,
        ] {
            for (source, required) in [
                (
                    "-- first query\nSELECT * FROM items;\nSELECT id FROM other_items;",
                    false,
                ),
                ("SELECT * FROM items;\nDELETE FROM items WHERE id=1;", true),
            ] {
                let (dir, imported) = prepared(source, dialect);
                let note = dir.path().join("authorization.note");
                let cancel = AtomicBool::new(false);
                imported.publish(&note, &cancel).unwrap();
                let mut reader = freeze(&note, &imported.script, &cancel).unwrap();
                let summary = reader.authorization_summary(0, 2, false, &cancel).unwrap();
                assert_eq!(summary.requires_authorization(), required);
            }
        }
        for summary in [
            AuthorizationSummary {
                session: 1,
                ..Default::default()
            },
            AuthorizationSummary {
                locks: 1,
                ..Default::default()
            },
            AuthorizationSummary {
                transactions: 1,
                ..Default::default()
            },
            AuthorizationSummary {
                risky: 1,
                ..Default::default()
            },
            AuthorizationSummary {
                ddl: 1,
                ..Default::default()
            },
        ] {
            assert!(summary.requires_authorization());
        }
    }
    #[test]
    fn leading_comments_do_not_hide_sql_or_change_the_frozen_source() {
        let source = "-- 说明; not a statement\r\n# more comment\r\n/* ordinary */\r\nDROP TRIGGER IF EXISTS `t`;";
        let (dir, imported) = prepared(source, SqlDialect::MySql);
        assert_eq!(imported.script.statements, 1);
        let entry = imported.page().unwrap().remove(0);
        assert_eq!(entry.line, 4);
        assert!(entry.has_comments);
        assert_eq!(entry.preview, "DROP TRIGGER IF EXISTS `t`;");
        let view = statement_preview(source.into(), entry.line, SqlDialect::MySql);
        assert_eq!((view.source_line, view.sql_line, view.end_line), (1, 4, 4));
        assert!(view.comments.contains("# more comment"));
        assert_eq!(view.source, source);
        assert_eq!(view.sql, "DROP TRIGGER IF EXISTS `t`;");
        let note = dir.path().join("comments.note");
        let cancel = AtomicBool::new(false);
        imported.publish(&note, &cancel).unwrap();
        let mut reader = freeze(&note, &imported.script, &cancel).unwrap();
        assert_eq!(reader.statement(0).unwrap(), source);
        for sql in [
            "SELECT '-- not a comment', '# value';",
            "--not_a_mysql_comment",
            "/*!40000 SET NAMES utf8mb4 */;",
            "/*M! SET NAMES utf8mb4 */;",
        ] {
            assert_eq!(leading_code_offset(sql, SqlDialect::MySql), 0);
        }
        assert_eq!(
            leading_code_offset("--comment\nSELECT 1", SqlDialect::Sqlite),
            10
        );
        assert_eq!(
            leading_code_offset("/* a /* b */ c */SELECT 1", SqlDialect::PostgreSql),
            17
        );
    }
    #[test]
    fn trigger_deletions_are_indexed_as_destructive_ddl_with_permission_location() {
        let (dir, imported) = prepared(
            "-- dump\nDROP TRIGGER IF EXISTS `one`;\nDROP TRIGGER IF EXISTS `two`;",
            SqlDialect::MySql,
        );
        assert_eq!(imported.script.unsupported, 0);
        let note = dir.path().join("triggers.note");
        let cancel = AtomicBool::new(false);
        imported.publish(&note, &cancel).unwrap();
        let mut reader = freeze(&note, &imported.script, &cancel).unwrap();
        let summary = reader.authorization_summary(0, 2, false, &cancel).unwrap();
        assert_eq!((summary.ddl, summary.destructive, summary.risky), (2, 2, 2));
        let blocked = permission_block(&mut reader, DatabaseKind::MySql, 0, &[])
            .unwrap()
            .unwrap();
        assert_eq!(blocked.command, "DROP TRIGGER");
        assert!(
            permission_block(
                &mut reader,
                DatabaseKind::MySql,
                0,
                &["script_drop_trigger".into()]
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn batch_authorization_counts_real_risks_and_pages_only_current_source() {
        let (dir, imported) = prepared(
            "USE app;\nSET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0;\nCREATE TABLE items(id INT);\nLOCK TABLES items WRITE;\nINSERT INTO items VALUES(1);\nUNLOCK TABLES;\nDROP TABLE items;\nSELECT 1;",
            SqlDialect::MySql,
        );
        let note = dir.path().join("batch.note");
        let cancel = AtomicBool::new(false);
        imported.publish(&note, &cancel).unwrap();
        let mut reader = freeze(&note, &imported.script, &cancel).unwrap();
        let summary = reader.authorization_summary(0, 8, false, &cancel).unwrap();
        assert_eq!(summary.session, 2);
        assert_eq!(summary.ddl, 2);
        assert_eq!(summary.writes, 1);
        assert_eq!(summary.locks, 2);
        assert_eq!(summary.reads, 1);
        assert_eq!(summary.destructive, 1);
        assert_eq!(summary.relaxed_checks, 1);
        assert_eq!(summary.risky, 6);
        assert_eq!(
            reader
                .authorization_page(0, 8, 0, false, false)
                .unwrap()
                .len(),
            6
        );
        let risks = reader.authorization_page(0, 8, 0, true, false).unwrap();
        assert_eq!(
            risks.iter().map(|r| r.number).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        assert!(
            reader
                .authorization_summary(0, 8, false, &AtomicBool::new(true))
                .is_err()
        );
    }
    #[test]
    fn permission_feedback_identifies_original_statement_line_and_category() {
        let (dir, imported) = prepared(
            "-- header\nSELECT 1;\n\nINSERT INTO items VALUES(1);\nDROP TABLE items;",
            SqlDialect::MySql,
        );
        let note = dir.path().join("permissions.note");
        let cancel = AtomicBool::new(false);
        imported.publish(&note, &cancel).unwrap();
        let mut reader = freeze(&note, &imported.script, &cancel).unwrap();
        let allowed = vec!["select".into()];
        assert!(
            permission_block(&mut reader, DatabaseKind::MySql, 0, &allowed)
                .unwrap()
                .is_none()
        );
        let block = permission_block(&mut reader, DatabaseKind::MySql, 1, &allowed)
            .unwrap()
            .unwrap();
        assert_eq!(block.number, 1);
        assert_eq!(block.line, 4);
        assert_eq!(block.command, "INSERT");
        let block = permission_block(&mut reader, DatabaseKind::MySql, 2, &allowed)
            .unwrap()
            .unwrap();
        assert_eq!(block.command, "DROP");
        assert!(
            permission_block(&mut reader, DatabaseKind::MySql, 1, &["insert".into()])
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn mysql_dump_enable_disable_keys_are_supported() {
        let (_dir, imported) = prepared(
            "/*!40000 ALTER TABLE `account` DISABLE KEYS */;\n/*!40000 ALTER TABLE `account` ENABLE KEYS */;",
            SqlDialect::MySql,
        );
        assert_eq!(imported.script.statements, 2);
        assert_eq!(imported.script.unsupported, 0);
        assert_eq!(imported.script.writes, 2);
    }
    #[test]
    fn issue_pages_preserve_original_numbers_and_reasons() {
        let (dir, imported) = prepared(
            "SELECT 1;\nSET GLOBAL sql_mode='';\nSELECT 2;\nDELIMITER $$\n",
            SqlDialect::MySql,
        );
        let note = dir.path().join("issues.note");
        imported.publish(&note, &AtomicBool::new(false)).unwrap();
        let issues = issue_page(&note, &imported.script, 0).unwrap();
        assert_eq!(
            issues.iter().map(|r| r.number).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(issues[0].line, 2);
        assert!(!issues[0].reason.is_empty());
        let source = (0..27)
            .map(|i| format!("SELECT {i};\n"))
            .collect::<String>();
        let (dir, imported) = prepared(&source, SqlDialect::MySql);
        let note = dir.path().join("pages.note");
        imported.publish(&note, &AtomicBool::new(false)).unwrap();
        assert_eq!(page(&note, &imported.script, 0).unwrap().len(), 6);
        assert_eq!(page(&note, &imported.script, 6).unwrap()[0].number, 6);
        assert_eq!(page(&note, &imported.script, 24).unwrap().len(), 3);
    }
    #[test]
    fn mysql_export_headers_and_footers_are_indexed_without_creating_cells() {
        let (_dir, imported) = prepared(
            "USE `creation_zone`;\n/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;\n/*!40101 SET NAMES utf8mb4 */;\nSET FOREIGN_KEY_CHECKS=0;\nLOCK TABLES `items` WRITE;\nINSERT INTO items VALUES(1);\nUNLOCK TABLES;\nSET FOREIGN_KEY_CHECKS=1;\n/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;",
            SqlDialect::MySql,
        );
        assert_eq!(imported.script.statements, 9);
        assert_eq!(imported.script.unsupported, 0);
        assert!(imported.page().unwrap().iter().all(|row| row.supported));
    }
    #[test]
    fn upgrades_existing_derived_indexes_without_changing_source() {
        let (dir, imported) = prepared(
            "USE app; SET NAMES utf8mb4; SET FOREIGN_KEY_CHECKS=0;",
            SqlDialect::MySql,
        );
        let note = dir.path().join("fixture.note");
        let cancel = AtomicBool::new(false);
        imported.publish(&note, &cancel).unwrap();
        let root = folder(&note).unwrap();
        let mut old = imported.script.clone();
        let mut bytes = fs::read(root.join(format!("{}.idx", old.index_hash))).unwrap();
        bytes[..8].copy_from_slice(b"TDBSQL01");
        bytes[56..64].copy_from_slice(&3u64.to_le_bytes());
        bytes[72..80].copy_from_slice(&0u64.to_le_bytes());
        for row in 0..3 {
            bytes[HEADER as usize + row * RECORD as usize + 24] = 0;
        }
        old.index_hash = format!("{:x}", Sha256::digest(&bytes));
        old.unsupported = 3;
        old.writes = 0;
        fs::write(root.join(format!("{}.idx", old.index_hash)), bytes).unwrap();
        let updated = refresh(&note, &old, &cancel).unwrap();
        assert_eq!(updated.source_hash, old.source_hash);
        assert_eq!(updated.name, old.name);
        assert_eq!(updated.unsupported, 0);
        assert_eq!(updated.statements, 3);
        assert_ne!(updated.index_hash, old.index_hash);
        assert!(freeze(&note, &updated, &cancel).is_ok());
    }
    fn prepared(text: &str, dialect: SqlDialect) -> (tempfile::TempDir, Prepared) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.sql");
        fs::write(&path, text).unwrap();
        let imported = prepare(&path, dialect, &AtomicBool::new(false), |_, _| {}).unwrap();
        (dir, imported)
    }
    #[test]
    fn lexical_boundaries_preserve_comments_literals_and_utf8() {
        for (dialect, text, count) in [
            (
                SqlDialect::MySql,
                "# ignore; DELETE FROM hidden;\nSELECT '汉字;''值'; -- ignore;\nSELECT 2;",
                2,
            ),
            (SqlDialect::MySql, "SELECT 1--2; SELECT 4;", 2),
            (
                SqlDialect::Sqlite,
                "SELECT [semi;column] FROM t; SELECT 2;",
                2,
            ),
            (
                SqlDialect::PostgreSql,
                "SELECT foo$bar FROM t; SELECT 2;",
                2,
            ),
            (
                SqlDialect::PostgreSql,
                "/* outer /* nested ; */ done */ SELECT $tag$a;b$tag$; SELECT $$x;y$$;",
                2,
            ),
            (
                SqlDialect::Sqlite,
                "SELECT 'a;''b'; SELECT \"semi;column\" FROM t; -- trailer",
                2,
            ),
        ] {
            let (_dir, prepared) = prepared(text, dialect);
            assert_eq!(prepared.script.statements, count);
            assert_eq!(prepared.script.unsupported, 0);
            let mut reader =
                Reader::from_root(prepared.directory.path(), &prepared.script).unwrap();
            let statements = (0..prepared.script.statements)
                .map(|number| reader.statement(number).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(statements.len(), count as usize);
            assert!(
                statements
                    .iter()
                    .all(
                        |sql| notebook_query::validate_script_statement(kind(dialect), sql).is_ok()
                    )
            );
        }
    }
    #[test]
    fn unsupported_statements_and_ambiguous_escaping_are_not_silently_split_for_execution() {
        for text in [
            "SET @x=dangerous_udf(); SELECT 1;",
            "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$",
            "SELECT `x\\`; DROP TABLE users; -- `;",
            "SELECT 1; /*!50000 DELETE FROM users */ SELECT 2;",
        ] {
            let (_dir, prepared) = prepared(text, SqlDialect::MySql);
            assert!(prepared.script.unsupported > 0);
        }
        let (_dir, prepared) = prepared(
            &format!("SELECT '{}'; SELECT 2;", "x".repeat(MAX_SQL_BYTES)),
            SqlDialect::Sqlite,
        );
        assert_eq!(prepared.script.unsupported, 1);
        assert!(prepared.page().unwrap()[0].bytes > MAX_SQL_BYTES as u64);
    }
    #[test]
    fn imports_reject_invalid_encoding_unterminated_input_and_support_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fixture.sql");
        for data in [
            vec![0xff],
            b"SELECT '\0';".to_vec(),
            b"SELECT 'unfinished".to_vec(),
            b"/* unfinished".to_vec(),
        ] {
            fs::write(&file, data).unwrap();
            assert!(
                prepare(
                    &file,
                    SqlDialect::Sqlite,
                    &AtomicBool::new(false),
                    |_, _| {}
                )
                .is_err()
            );
        }
        fs::write(&file, b"\xef\xbb\xbfSELECT 1;").unwrap();
        let imported = prepare(
            &file,
            SqlDialect::Sqlite,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(
            Reader::from_root(imported.directory.path(), &imported.script)
                .unwrap()
                .statement(0)
                .unwrap(),
            "SELECT 1;"
        );
        assert!(prepare(&file, SqlDialect::Sqlite, &AtomicBool::new(true), |_, _| {}).is_err());
    }
    #[test]
    fn frozen_statement_locations_preserve_source_lines() {
        let (dir, prepared) = prepared(
            "SELECT 1;\n\n-- failed query\nSELECT 2;",
            SqlDialect::Sqlite,
        );
        let note = dir.path().join("locations.note");
        let cancel = AtomicBool::new(false);
        prepared.publish(&note, &cancel).unwrap();
        let mut reader = freeze(&note, &prepared.script, &cancel).unwrap();
        assert_eq!(reader.statement_line(0).unwrap(), 1);
        assert_eq!(reader.statement_line(1).unwrap(), 4);
        assert!(reader.statement_line(2).is_err());
    }
    #[test]
    fn resources_are_frozen_verified_and_copied_with_notes() {
        let (dir, prepared) = prepared(
            "CREATE TABLE t(id INT); INSERT INTO t VALUES(1); SELECT * FROM t;",
            SqlDialect::Sqlite,
        );
        assert_eq!(prepared.script.writes, 2);
        let path = dir.path().join("fixture.note");
        let cancel = AtomicBool::new(false);
        prepared.publish(&path, &cancel).unwrap();
        let mut frozen = freeze(&path, &prepared.script, &cancel).unwrap();
        let first = frozen.statement(0).unwrap();
        let mut note = turbodbn_core::Note::new("fixture");
        note.cells.push(turbodbn_core::Cell::new(
            "",
            turbodbn_core::CellContent::SqlScript {
                script: prepared.script.clone(),
                connection_id: None,
                database: None,
            },
        ));
        assert!(note.to_json().unwrap().len() < 2048);
        crate::note_store::save(&path, &note, None).unwrap();
        let other = dir.path().join("other/fixture.note");
        copy_resources(&path, &other, &note).unwrap();
        assert_eq!(preview(&other, &prepared.script, 0).unwrap(), first);
        let archive = crate::note_store::archive(&path, None).unwrap();
        assert_eq!(preview(&archive, &prepared.script, 0).unwrap(), first);
        fs::write(
            folder(&other)
                .unwrap()
                .join(format!("{}.sql", prepared.script.source_hash)),
            b"DELETE FROM t;",
        )
        .unwrap();
        assert!(freeze(&other, &prepared.script, &cancel).is_err());
        assert_eq!(frozen.statement(0).unwrap(), first);
    }
    #[test]
    fn a_hundred_thousand_statements_use_a_paged_disk_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.sql");
        let mut file = BufWriter::new(File::create(&path).unwrap());
        let sql = format!("SELECT '{}';\n", "x".repeat(950));
        for _ in 0..100_000 {
            file.write_all(sql.as_bytes()).unwrap();
        }
        file.flush().unwrap();
        drop(file);
        let imported = prepare(
            &path,
            SqlDialect::Sqlite,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(imported.script.statements, 100_000);
        assert_eq!(imported.script.unsupported, 0);
        let page = imported.page().unwrap();
        assert_eq!(page.len(), PAGE_SIZE as usize);
        assert!(page.iter().all(|entry| entry.preview.len() <= 512));
        assert_eq!(
            fs::metadata(
                imported
                    .directory
                    .path()
                    .join(format!("{}.idx", imported.script.index_hash))
            )
            .unwrap()
            .len(),
            HEADER + 100_000 * RECORD
        );
    }
}

#[cfg(test)]
mod editor_range_tests {
    #[test]
    fn query_units_run_whole_source_or_complete_selection_and_plan_current_statement() {
        use super::{SqlDialect, editor_query_target};
        for dialect in [
            SqlDialect::MySql,
            SqlDialect::PostgreSql,
            SqlDialect::Sqlite,
        ] {
            let sql = "-- first; comment\nSELECT ';';\nSELECT 2;\nSELECT 3;";
            let cursor = sql.find("SELECT 2").unwrap();
            let all = editor_query_target(sql, dialect, cursor..cursor, cursor, false).unwrap();
            assert_eq!(all.len(), 3);
            let selected =
                editor_query_target(sql, dialect, all[1].clone(), cursor, false).unwrap();
            assert_eq!(selected, vec![all[1].clone()]);
            let pair =
                editor_query_target(sql, dialect, all[1].start..all[2].end, cursor, false).unwrap();
            assert_eq!(pair.len(), 2);
            let plan = editor_query_target(sql, dialect, cursor..cursor, cursor, true).unwrap();
            assert_eq!(plan, vec![all[1].clone()]);
            assert!(editor_query_target(sql, dialect, cursor..cursor + 3, cursor, false).is_err());
        }
    }
    #[test]
    fn unified_query_segmentation_preserves_session_commands_and_large_statements() {
        use super::{SqlDialect, editor_query_target};
        let sql = "USE app; SET NAMES utf8mb4; SELECT 1;";
        assert_eq!(
            editor_query_target(sql, SqlDialect::MySql, 0..0, 0, false)
                .unwrap()
                .len(),
            3
        );
        let sql = format!("SELECT '{}'; SELECT 2;", "x".repeat(70_000));
        let ranges = editor_query_target(&sql, SqlDialect::Sqlite, 0..0, 0, false).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            ranges
                .iter()
                .map(|range| &sql[range.clone()])
                .collect::<String>(),
            sql
        );
    }
    use super::*;
    #[test]
    fn statements_ignore_quoted_and_comment_semicolons_and_preserve_exact_source() {
        let sql = "# SELECT * FROM `库`.`ignored`;\nSELECT ';' AS value;\n-- comment ;\nSELECT * FROM `库`.`cq_itemtype`;\nSELECT 3; -- trailing";
        let ranges = editor_ranges(sql, SqlDialect::MySql).unwrap();
        assert_eq!(ranges.len(), 3);
        assert_eq!(
            ranges.iter().map(|r| &sql[r.clone()]).collect::<String>(),
            sql
        );
        let cursor = sql.find("cq_itemtype").unwrap();
        let (selected, _) = editor_target(sql, SqlDialect::MySql, cursor..cursor, cursor).unwrap();
        assert!(sql[selected[0].clone()].contains("cq_itemtype"));
        assert!(!sql[selected[0].clone()].contains("SELECT 3"));
        assert_eq!(
            editor_target(sql, SqlDialect::MySql, 0..sql.len(), sql.len())
                .unwrap()
                .0
                .len(),
            3
        );
        assert!(editor_target(sql, SqlDialect::MySql, cursor..cursor + 2, cursor).is_err());
    }
    #[test]
    fn postgres_dollar_quotes_and_unterminated_input_are_not_naively_split() {
        let sql = "SELECT $$a;b$$; /* x;y */ SELECT '中';";
        assert_eq!(editor_ranges(sql, SqlDialect::PostgreSql).unwrap().len(), 2);
        assert!(editor_ranges("SELECT 'unterminated", SqlDialect::MySql).is_err());
        assert_eq!(
            editor_ranges("SELECT ';'; SELECT 2;", SqlDialect::Sqlite)
                .unwrap()
                .len(),
            2
        );
    }
}

#[cfg(test)]
mod text_attachment_tests {
    use super::*;
    #[test]
    fn editor_text_prepares_and_publishes_with_no_open_write_handle() {
        let directory = tempfile::tempdir().unwrap();
        let note = directory.path().join("source.note");
        let source = "-- keep comment;\nSELECT ';';\nSELECT 2; -- trailing";
        let cancel = AtomicBool::new(false);
        let prepared = prepare_text(source, SqlDialect::Sqlite, &cancel).unwrap();
        assert_eq!(prepared.script.statements, 2);
        prepared.publish(&note, &cancel).unwrap();
        let file = directory
            .path()
            .join(".turbodbnote-scripts")
            .join(format!("{}.sql", prepared.script.source_hash));
        assert_eq!(fs::read_to_string(file).unwrap(), source);
    }
}
