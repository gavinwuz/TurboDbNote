//! Atomic, bounded document IO. Notebook contents never enter logs or error messages.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use turbodbn_core::Note;
use uuid::Uuid;

pub const MAX_NOTE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_CELLS: usize = 128;
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub path: PathBuf,
    pub title: String,
    pub project: String,
    pub draft: bool,
    pub used: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    pub version: u32,
    pub entries: Vec<Entry>,
    pub open: Vec<PathBuf>,
    pub roots: Vec<PathBuf>,
}
impl Default for Index {
    fn default() -> Self {
        Self {
            version: 1,
            entries: vec![],
            open: vec![],
            roots: vec![],
        }
    }
}
pub struct Loaded {
    pub note: Note,
    pub hash: [u8; 32],
}
fn io_error(_: impl std::fmt::Display) -> String {
    "笔记文件操作失败，请检查目录权限、磁盘空间及文件是否被其他程序占用".into()
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > MAX_NOTE_BYTES {
        return Err("笔记文件超过 16 MiB 上限".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_NOTE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_NOTE_BYTES {
        return Err("笔记文件超过 16 MiB 上限".into());
    }
    Ok(bytes)
}
pub fn load(path: &Path) -> Result<Loaded, String> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("note"))
    {
        return Err("请选择 .note 文件".into());
    }
    let bytes = read_bytes(path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "笔记不是有效 UTF-8 文件")?;
    let note = Note::from_json(text).map_err(|_| "笔记格式损坏或版本不受支持；原文件已保留")?;
    if note.cells.len() > MAX_CELLS {
        return Err("单份笔记最多支持 128 个子区域".into());
    }
    Ok(Loaded {
        note,
        hash: Sha256::digest(bytes).into(),
    })
}
pub fn save(path: &Path, note: &Note, expected: Option<[u8; 32]>) -> Result<[u8; 32], String> {
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("note"))
    {
        return Err("请选择 .note 文件的绝对路径".into());
    }
    if note.cells.len() > MAX_CELLS {
        return Err("单份笔记最多支持 128 个子区域".into());
    }
    let text = note.to_json().map_err(|error| match error {
        turbodbn_core::notebook::NoteError::ScriptTextTooLarge => {
            "可编辑 SQL 脚本超过 1 MiB 上限，请拆分或导入为脚本附件"
        }
        _ => "笔记内容无法保存，请检查子区域标识及格式版本",
    })?;
    if text.len() as u64 > MAX_NOTE_BYTES {
        return Err("笔记文件超过 16 MiB 上限".into());
    }
    let parent = path.parent().ok_or("笔记目录无效")?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    staged.write_all(text.as_bytes()).map_err(io_error)?;
    staged.as_file().sync_all().map_err(io_error)?;
    if let Some(expected) = expected {
        let current: [u8; 32] = Sha256::digest(read_bytes(path)?).into();
        if current != expected {
            return Err(
                "笔记已被其他程序修改，请另存为新文件或重新打开；外部内容没有被覆盖".into(),
            );
        }
        staged.persist(path).map_err(io_error)?;
    } else {
        staged.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                "目标笔记已存在，请选择其他文件名".to_string()
            } else {
                io_error(error)
            }
        })?;
    }
    Ok(Sha256::digest(text.as_bytes()).into())
}
/// Recoverable deletion on the same filesystem. Keep the original basename for manual recovery.
pub fn archive(path: &Path, expected: Option<[u8; 32]>) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !path.is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("只能删除普通 .note 文件".into());
    }
    let loaded = load(path)?;
    if expected.is_some_and(|hash| hash != loaded.hash) {
        return Err("笔记已被其他程序修改，请重新打开后再删除".into());
    }
    let folder = path
        .parent()
        .ok_or("笔记目录无效")?
        .join(".turbodbnote-trash");
    fs::create_dir_all(&folder).map_err(io_error)?;
    let archived = folder.join(format!(
        "{}-{}",
        uuid::Uuid::new_v4(),
        path.file_name().ok_or("笔记文件名无效")?.to_string_lossy()
    ));
    crate::sql_script::copy_resources(path, &archived, &loaded.note)?;
    fs::rename(path, &archived).map_err(io_error)?;
    Ok(archived)
}

pub fn load_index(path: &Path) -> Result<Index, String> {
    if !path.exists() {
        return Ok(Index {
            version: 1,
            ..Default::default()
        });
    }
    let bytes = read_bytes(path)?;
    let index: Index = serde_json::from_slice(&bytes).map_err(|_| "笔记索引损坏，原文件已保留")?;
    if index.version != 1 {
        return Err("笔记索引版本不受支持".into());
    }
    Ok(index)
}
pub fn save_index(path: &Path, index: &Index) -> Result<(), String> {
    let parent = path.parent().ok_or("索引目录无效")?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let bytes = serde_json::to_vec(index).map_err(io_error)?;
    if bytes.len() as u64 > MAX_NOTE_BYTES || index.entries.len() > 2000 {
        return Err("笔记索引超过上限，请整理笔记列表".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.as_file().sync_all().map_err(io_error)?;
    file.persist(path).map_err(io_error)?;
    Ok(())
}
/// Reserve a daily, human-readable filename without overwriting another document.
pub fn create_dated(
    directory: &Path,
    date: &str,
    mut note: Note,
) -> Result<(PathBuf, Loaded), String> {
    if date.len() != 6 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Invalid note date".into());
    }
    fs::create_dir_all(directory).map_err(io_error)?;
    let readable = note
        .title
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '-'
            } else {
                ch
            }
        })
        .take(60)
        .collect::<String>()
        .trim_matches(['.', ' ', '-'])
        .to_owned();
    let mut readable = readable;
    while readable.len() > 160 {
        readable.pop();
    }
    let readable = if readable.is_empty() {
        "笔记".to_owned()
    } else {
        readable
    };
    let prefix = format!("{readable}-{date}-");
    let mut next = 1u64;
    for item in fs::read_dir(directory).map_err(io_error)? {
        let path = item.map_err(io_error)?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("note"))
            && let Some(n) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| {
                    s.rsplit_once(&format!("-{date}-"))
                        .map(|(_, number)| number)
                })
                .and_then(|s| s.parse::<u64>().ok())
        {
            next = next.max(n.saturating_add(1));
        }
    }
    let generated = note.title.trim().is_empty();
    for _ in 0..64 {
        let name = format!("{prefix}{next}");
        let path = directory.join(format!("{name}.note"));
        if generated {
            note.title = format!("笔记-{date}-{next}");
        }
        match save(&path, &note, None) {
            Ok(hash) => return Ok((path, Loaded { note, hash })),
            Err(_) if path.exists() => next = next.saturating_add(1),
            Err(error) => return Err(error),
        }
    }
    Err(io_error("filename contention"))
}

pub fn discover(index: &mut Index, directory: &Path) -> Result<(), String> {
    if !directory.is_absolute() {
        return Err("临时笔记目录必须是绝对路径".into());
    }
    fs::create_dir_all(directory).map_err(io_error)?;
    if !index.roots.contains(&directory.to_path_buf()) {
        index.roots.push(directory.to_path_buf());
    }
    if index.entries.len() > 2000 {
        return Err("笔记列表超过 2000 项，请整理索引".into());
    }
    let roots = index.roots.clone();
    for root in roots {
        if !root.exists() {
            continue;
        }
        let mut seen = 0;
        for item in fs::read_dir(root).map_err(io_error)? {
            let path = item.map_err(io_error)?.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("note"))
            {
                continue;
            }
            seen += 1;
            if seen > 2000 {
                return Err("临时笔记目录超过 2000 个文件，请整理目录后重试".into());
            }
            if index.entries.iter().any(|e| e.path == path) {
                continue;
            }
            // A damaged file is left intact and can be explicitly opened for an error report.
            if let Ok(loaded) = load(&path) {
                if index.entries.len() >= 2000 {
                    return Err("笔记列表达到 2000 项上限，请整理目录".into());
                }
                index.entries.push(Entry {
                    id: loaded.note.id,
                    path,
                    title: loaded.note.title,
                    project: loaded.note.project,
                    draft: true,
                    used: 0,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daily_names_are_monotonic_preserve_titles_and_never_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let (first, loaded) = create_dated(root.path(), "261022", Note::new("")).unwrap();
        assert_eq!(first.file_name().unwrap(), "笔记-261022-1.note");
        assert_eq!(loaded.note.title, "笔记-261022-1");
        let before = fs::read(&first).unwrap();
        let (table, loaded) = create_dated(root.path(), "261022", Note::new("cq_user")).unwrap();
        assert_eq!(table.file_name().unwrap(), "cq_user-261022-2.note");
        assert_eq!(loaded.note.title, "cq_user");
        fs::remove_file(table).unwrap();
        let (next, _) = create_dated(root.path(), "261023", Note::new("")).unwrap();
        assert_eq!(next.file_name().unwrap(), "笔记-261023-1.note");
        assert_eq!(fs::read(first).unwrap(), before);
        assert!(create_dated(root.path(), "../bad", Note::new("")).is_err());
    }
    #[test]
    fn readable_title_names_preserve_title_and_remove_windows_path_punctuation() {
        let root = tempfile::tempdir().unwrap();
        let title = "师徒 · 131.165:3306 / 查询";
        let (path, loaded) = create_dated(root.path(), "261026", Note::new(title)).unwrap();
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("师徒 · 131.165-3306 - 查询-261026-")
        );
        assert_eq!(loaded.note.title, title);
        assert_eq!(path.parent().unwrap(), root.path());
    }
    #[test]
    fn simultaneous_daily_note_creation_uses_distinct_files() {
        let root = tempfile::tempdir().unwrap();
        let jobs = (0..4)
            .map(|_| {
                let path = root.path().to_owned();
                std::thread::spawn(move || create_dated(&path, "261022", Note::new("")).unwrap().0)
            })
            .collect::<Vec<_>>();
        let paths = jobs
            .into_iter()
            .map(|job| job.join().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(paths.len(), 4);
        for path in paths {
            assert!(load(&path).is_ok());
        }
    }
    #[test]
    fn archived_notes_are_recoverable_and_changed_files_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.note");
        let note = Note::new("Keep me");
        let hash = save(&path, &note, None).unwrap();
        assert!(archive(&path, Some([0; 32])).is_err());
        assert!(path.exists());
        let archived = archive(&path, Some(hash)).unwrap();
        assert!(!path.exists());
        assert_eq!(load(&archived).unwrap().note, note);
        let mut index = Index {
            version: 1,
            ..Default::default()
        };
        discover(&mut index, root.path()).unwrap();
        assert!(index.entries.is_empty());
        fs::rename(archived, &path).unwrap();
        assert_eq!(load(&path).unwrap().hash, hash);
    }
    #[test]
    fn changing_directory_preserves_old_drafts_and_open_paths() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        let note = Note::new("draft");
        let path = first.join(format!("{}.note", note.id));
        save(&path, &note, None).unwrap();
        let mut index = Index {
            version: 1,
            ..Default::default()
        };
        discover(&mut index, &first).unwrap();
        index.open.push(path.clone());
        discover(&mut index, &second).unwrap();
        let manifest = root.path().join("library.json");
        save_index(&manifest, &index).unwrap();
        let restored = load_index(&manifest).unwrap();
        assert_eq!(restored.open, vec![path.clone()]);
        assert_eq!(restored.roots, vec![first, second]);
        assert_eq!(load(&path).unwrap().note.title, "draft");
    }
    #[test]
    fn unnamed_document_survives_without_index_and_external_changes_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.note");
        let mut note = Note::welcome();
        note.title.clear();
        note.project = "shared project".into();
        let hash = save(&path, &note, None).unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.note == note);
        let mut index = Index {
            version: 1,
            ..Default::default()
        };
        discover(&mut index, root.path()).unwrap();
        assert!(index.entries.len() == 1 && index.entries[0].project == note.project);
        let mut external = note.clone();
        external.title = "external".into();
        save(&path, &external, Some(hash)).unwrap();
        note.title = "local".into();
        assert!(save(&path, &note, Some(hash)).is_err());
        assert!(load(&path).unwrap().note == external);
        assert!(save(&path, &note, None).is_err());
    }
    #[test]
    fn malformed_or_future_notes_are_never_rewritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("future.note");
        let bytes = b"{\"schema_version\":99}";
        fs::write(&path, bytes).unwrap();
        assert!(load(&path).is_err());
        assert!(fs::read(path).unwrap() == bytes);
    }
}
