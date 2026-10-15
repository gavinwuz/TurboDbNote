//! Local capability ceiling. This never grants server privileges or bypasses global policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccessMode {
    #[default]
    ReadOnly,
    ReadWrite,
    Admin,
}
impl AccessMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "mcp.mode.read",
            Self::ReadWrite => "mcp.mode.write",
            Self::Admin => "mcp.mode.admin",
        }
    }
    pub fn allows(self, command: &str) -> bool {
        use crate::database::notebook_query::{COMMANDS, SCRIPT_COMMANDS};
        match command {
            "select" | "explain" | "show_create" | "show_columns" => true,
            "insert" | "update" | "delete" => self != Self::ReadOnly,
            _ => {
                self == Self::Admin
                    && COMMANDS
                        .iter()
                        .chain(SCRIPT_COMMANDS)
                        .any(|(key, _, _)| *key == command)
            }
        }
    }
    pub fn allows_note(self, command: &str) -> bool {
        self.allows(command) && (command != "delete" || self == Self::Admin)
    }
    pub fn allows_note_effective(self, command: &str, globally_allowed: bool) -> bool {
        globally_allowed && self.allows_note(command)
    }
    pub fn allows_effective(self, command: &str, globally_allowed: bool) -> bool {
        globally_allowed && self.allows(command)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_modes_never_expand_global_commands() {
        for mode in [
            AccessMode::ReadOnly,
            AccessMode::ReadWrite,
            AccessMode::Admin,
        ] {
            for (key, _, _) in crate::database::notebook_query::COMMANDS
                .iter()
                .chain(crate::database::notebook_query::SCRIPT_COMMANDS)
            {
                assert!(!mode.allows_effective(key, false));
            }
        }
        assert!(!AccessMode::ReadOnly.allows("insert"));
        assert!(!AccessMode::ReadWrite.allows_note("delete"));
        assert!(AccessMode::ReadWrite.allows_note("insert"));
        assert!(AccessMode::ReadWrite.allows_note("update"));
        assert!(AccessMode::Admin.allows_note("delete"));
        assert!(!AccessMode::Admin.allows_note_effective("delete", false));
        assert!(AccessMode::ReadWrite.allows("delete"));
        use crate::{DatabaseKind, database::notebook_query::note_mode_allows};
        assert!(!note_mode_allows(
            AccessMode::ReadWrite,
            DatabaseKind::MySql,
            "REPLACE INTO t VALUES (1)",
            true
        ));
        assert!(note_mode_allows(
            AccessMode::Admin,
            DatabaseKind::MySql,
            "REPLACE INTO t VALUES (1)",
            true
        ));
        assert!(!note_mode_allows(
            AccessMode::ReadWrite,
            DatabaseKind::Sqlite,
            "INSERT OR REPLACE INTO t VALUES(1)",
            false
        ));
        assert!(note_mode_allows(
            AccessMode::ReadWrite,
            DatabaseKind::MySql,
            "INSERT INTO t VALUES(1) ON DUPLICATE KEY UPDATE id=id",
            true
        ));

        assert!(AccessMode::ReadWrite.allows("insert"));
        assert!(!AccessMode::ReadWrite.allows("script_drop_trigger"));
        assert!(AccessMode::Admin.allows("script_drop_trigger"));
        assert!(!AccessMode::Admin.allows("grant"));
    }
}
