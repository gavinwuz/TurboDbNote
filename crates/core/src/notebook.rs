use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub schema_version: u32,
    pub id: Uuid,
    pub title: String,
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
        status: TaskStatus,
        due_at: Option<i64>,
        remind_at: Option<i64>,
        linked_cells: Vec<Uuid>,
        connection_id: Option<Uuid>,
    },
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
}

impl Note {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            title: title.into(),
            cells: vec![],
        }
    }

    pub fn from_json(json: &str) -> Result<Self, NoteError> {
        // Check the envelope before decoding version-specific cell fields.
        let value: serde_json::Value = serde_json::from_str(json)?;
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let version: Version = serde_json::from_value(value.clone())?;
        if version.schema_version != SCHEMA_VERSION {
            return Err(NoteError::UnsupportedVersion(version.schema_version));
        }
        let note: Self = serde_json::from_value(value)?;
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
        let mut ids = HashSet::new();
        for cell in &self.cells {
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
    fn duplicate_ids_are_rejected() {
        let mut note = Note::welcome();
        note.cells.push(note.cells[0].clone());
        assert!(matches!(note.to_json(), Err(NoteError::DuplicateCell(_))));
    }
}
