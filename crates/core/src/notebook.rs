use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub schema_version: u32,
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub default_connection_id: Option<Uuid>,
    #[serde(default)]
    pub default_database: Option<String>,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cell {
    pub id: Uuid,
    pub title: String,
    pub content: CellContent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CellContent {
    Sql {
        source: String,
        connection_id: Option<Uuid>,
        database: Option<String>,
    },
    Markdown {
        source: String,
    },
    Task {
        description: String,
        #[serde(default)]
        items: Vec<TaskItem>,
        status: TaskStatus,
        due_at: Option<i64>,
        remind_at: Option<i64>,
        linked_cells: Vec<Uuid>,
        connection_id: Option<Uuid>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskItem {
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub completed: bool,
    pub due_at: Option<i64>,
    /// Minutes before the deadline; None disables reminders.
    pub reminder_minutes: Option<i64>,
    #[serde(default)]
    pub reminded_at: Option<i64>,
}
impl TaskItem {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            assignee: String::new(),
            completed: false,
            due_at: None,
            reminder_minutes: None,
            reminded_at: None,
        }
    }
    pub fn reminder_at(&self) -> Option<i64> {
        if self.completed {
            return None;
        }
        self.due_at?
            .checked_sub(self.reminder_minutes?.checked_mul(60)?)
    }
    pub fn needs_reminder(&self, now: i64) -> bool {
        self.reminder_at()
            .is_some_and(|at| at <= now && self.reminded_at != Some(at))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error("无法解析 .note 文件：{0}")]
    Json(#[from] serde_json::Error),
    #[error("不支持的 .note 格式版本：{0}")]
    UnsupportedVersion(u32),
    #[error("子项 ID 重复：{0}")]
    DuplicateCell(Uuid),
    #[error("笔记标题或项目名称过长")]
    MetadataTooLong,
    #[error("任务项数据无效或超过上限")]
    InvalidTask,
}

impl Note {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            title: title.into(),
            project: String::new(),
            default_connection_id: None,
            default_database: None,
            cells: vec![],
        }
    }

    pub fn from_json(json: &str) -> Result<Self, NoteError> {
        // Skip cell payloads without allocating a Value tree; reject future versions first.
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let version: Version = serde_json::from_str(json)?;
        if ![1, SCHEMA_VERSION].contains(&version.schema_version) {
            return Err(NoteError::UnsupportedVersion(version.schema_version));
        }
        let mut note: Self = serde_json::from_str(json)?;
        note.schema_version = SCHEMA_VERSION;
        note.validate()?;
        Ok(note)
    }

    pub fn to_json(&self) -> Result<String, NoteError> {
        self.validate()?;
        Ok(serde_json::to_string_pretty(self)?)
    }

    fn validate(&self) -> Result<(), NoteError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(NoteError::UnsupportedVersion(self.schema_version));
        }
        if self.title.len() > 1024 || self.project.len() > 1024 {
            return Err(NoteError::MetadataTooLong);
        }
        let mut ids = HashSet::new();
        for cell in &self.cells {
            if let CellContent::Task { items, .. } = &cell.content {
                if items.len() > 1000 {
                    return Err(NoteError::InvalidTask);
                }
                for item in items {
                    if !ids.insert(item.id)
                        || item.title.len() > 8192
                        || item.assignee.len() > 1024
                        || item
                            .reminder_minutes
                            .is_some_and(|v| !(0..=10080).contains(&v))
                    {
                        return Err(NoteError::InvalidTask);
                    }
                }
            }
            if !ids.insert(cell.id) {
                return Err(NoteError::DuplicateCell(cell.id));
            }
        }
        Ok(())
    }

    pub fn welcome() -> Self {
        let mut note = Self::new("入门工作台.note");
        note.cells = vec![
            Cell::new("探索数据", CellContent::Sql {
                source: "-- 选择连接后，在这里开始查询\nSELECT id, name, status\nFROM customers\nLIMIT 100;".into(),
                connection_id: None,
                database: None,
            }),
            Cell::new("分析笔记", CellContent::Markdown {
                source: "# 查询记录\n\n在这里记录分析过程与结论。\n\n- SQL 结果展示在对应子项下方\n- 数据库结构位于右侧面板".into(),
            }),
            Cell::new("验证查询口径", CellContent::Task {
                description: "核对数据范围，记录分析结论。".into(),
                items:vec![],
                status: TaskStatus::Pending,
                due_at: None,
                remind_at: None,
                linked_cells: vec![],
                connection_id: None,
            }),
        ];
        note
    }
}

impl Cell {
    /// A copied region is a new document identity, never a running query clone.
    pub fn duplicate(&self) -> Self {
        let mut copied = self.clone();
        copied.id = Uuid::new_v4();
        if let CellContent::Task {
            items,
            linked_cells,
            ..
        } = &mut copied.content
        {
            for item in items {
                item.id = Uuid::new_v4();
                item.reminded_at = None;
            }
            for id in linked_cells {
                if *id == self.id {
                    *id = copied.id;
                }
            }
        }
        copied
    }
    pub fn empty_sibling(&self) -> Self {
        let content = match &self.content {
            CellContent::Sql {
                connection_id,
                database,
                ..
            } => CellContent::Sql {
                source: String::new(),
                connection_id: *connection_id,
                database: database.clone(),
            },
            CellContent::Markdown { .. } => CellContent::Markdown {
                source: String::new(),
            },
            CellContent::Task { .. } => CellContent::Task {
                description: String::new(),
                items: vec![],
                status: TaskStatus::Pending,
                due_at: None,
                remind_at: None,
                linked_cells: vec![],
                connection_id: None,
            },
        };
        Self::new("", content)
    }
    pub fn new(title: impl Into<String>, content: CellContent) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checklist_migration_roundtrip_and_copy_keep_independent_identity() {
        let legacy = r#"{"schema_version":1,"id":"c19902b9-30ef-4657-8d14-b6ef704a0261","title":"task","cells":[{"id":"73e6f2f0-e92c-419b-a7cb-a385af974be7","title":"legacy","content":{"type":"task","description":"- first\n- second","status":"in_progress","due_at":null,"remind_at":null,"linked_cells":[],"connection_id":null}}]}"#;
        let mut note = Note::from_json(legacy).unwrap();
        assert_eq!(note.schema_version, 2);
        let CellContent::Task {
            items, description, ..
        } = &mut note.cells[0].content
        else {
            panic!("task expected")
        };
        assert!(items.is_empty());
        assert_eq!(description, "- first\n- second");
        let mut item = TaskItem::new("first");
        item.assignee = "owner".into();
        item.due_at = Some(2000);
        item.reminder_minutes = Some(5);
        items.push(item);
        assert_eq!(Note::from_json(&note.to_json().unwrap()).unwrap(), note);
        let duplicate = note.cells[0].duplicate();
        let CellContent::Task { items: copied, .. } = duplicate.content else {
            panic!("task expected")
        };
        let CellContent::Task {
            items: original, ..
        } = &note.cells[0].content
        else {
            panic!("task expected")
        };
        assert_ne!(copied[0].id, original[0].id);
        assert!(copied[0].reminded_at.is_none());
    }
    #[test]
    fn reminders_deduplicate_complete_and_reschedule_without_overflow() {
        let mut item = TaskItem::new("task");
        item.due_at = Some(2000);
        item.reminder_minutes = Some(5);
        assert!(!item.needs_reminder(1699));
        assert!(item.needs_reminder(1700));
        item.reminded_at = item.reminder_at();
        assert!(!item.needs_reminder(1800));
        item.due_at = Some(2200);
        assert!(item.needs_reminder(1900));
        item.completed = true;
        assert!(!item.needs_reminder(9999));
        item.completed = false;
        item.due_at = Some(i64::MIN);
        assert_eq!(item.reminder_at(), None);
    }
    #[test]
    fn note_defaults_round_trip_without_rebinding_existing_cells() {
        let mut note = Note::welcome();
        let original = note.cells.clone();
        note.default_connection_id = Some(Uuid::new_v4());
        note.default_database = Some("analytics".into());
        let restored = Note::from_json(&note.to_json().unwrap()).unwrap();
        assert_eq!(restored.cells, original);
        assert_eq!(restored.default_connection_id, note.default_connection_id);
        let mut legacy = serde_json::to_value(note).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("default_connection_id");
        legacy.as_object_mut().unwrap().remove("default_database");
        let restored = Note::from_json(&legacy.to_string()).unwrap();
        assert!(restored.default_connection_id.is_none() && restored.default_database.is_none());
    }

    #[test]
    fn copied_and_inserted_regions_have_distinct_ids_and_preserve_sql_binding() {
        let original = Cell::new(
            "annotation",
            CellContent::Sql {
                source: "SELECT 42".into(),
                connection_id: Some(Uuid::new_v4()),
                database: Some("analytics".into()),
            },
        );
        let copy = original.duplicate();
        let blank = original.empty_sibling();
        assert_ne!(copy.id, original.id);
        assert_ne!(blank.id, copy.id);
        assert_eq!(copy.content, original.content);
        if let CellContent::Sql {
            source,
            connection_id,
            database,
        } = blank.content
        {
            assert!(source.is_empty());
            if let CellContent::Sql {
                connection_id: id,
                database: db,
                ..
            } = original.content
            {
                assert_eq!(connection_id, id);
                assert_eq!(database, db);
            }
        } else {
            panic!("SQL sibling must remain SQL");
        }
    }
    #[test]
    fn all_cell_types_round_trip_without_runtime_data() {
        let note = Note::welcome();
        let json = note.to_json().unwrap();
        assert_eq!(Note::from_json(&json).unwrap(), note);
        assert!(!json.contains("query_state"));
    }

    #[test]
    fn future_schema_is_rejected_before_decoding_cells() {
        assert!(matches!(
            Note::from_json(r#"{"schema_version":99,"cells":"future format"}"#),
            Err(NoteError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn bounded_large_document_and_reordered_version_round_trip() {
        let mut note = Note::new("large");
        note.cells.push(Cell::new(
            "",
            CellContent::Markdown {
                source: "中文\n\"\\".repeat(100_000),
            },
        ));
        let text = note.to_json().unwrap();
        assert_eq!(Note::from_json(&text).unwrap(), note);
        assert!(matches!(
            Note::from_json(r#"{"cells":[{"future":true}],"schema_version":99}"#),
            Err(NoteError::UnsupportedVersion(99))
        ));
        assert!(Note::from_json(r#"{"schema_version":1,"schema_version":99}"#).is_err());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut note = Note::welcome();
        note.cells.push(note.cells[0].clone());
        assert!(matches!(note.to_json(), Err(NoteError::DuplicateCell(_))));
    }
}
