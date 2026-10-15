//! Actionable permission feedback. Changing a ceiling never executes SQL.
use crate::{
    locale::{t, tr},
    preferences::Preferences,
};
use gpui::*;
use turbodbn_services::{access_mode::AccessMode, database::notebook_query, mysql::Profile};
#[derive(Clone, Debug)]
pub enum Advice {
    Mode {
        current: AccessMode,
        required: AccessMode,
        command: String,
        key: String,
    },
    Global {
        command: String,
    },
    Connection,
    Unsupported,
}
fn label(command: &str) -> String {
    notebook_query::COMMANDS
        .iter()
        .chain(notebook_query::SCRIPT_COMMANDS)
        .find(|(key, _, _)| *key == command)
        .map(|(_, label, _)| (*label).to_owned())
        .unwrap_or_else(|| "SQL".into())
}
impl Advice {
    pub fn for_command(mode: AccessMode, profile: &Profile, key: &str, cx: &App) -> Option<Self> {
        if !cx.global::<Preferences>().database.command_enabled(key) {
            return Some(Self::Global {
                command: label(key),
            });
        }
        if matches!(
            key,
            "insert"
                | "update"
                | "delete"
                | "create"
                | "alter"
                | "drop"
                | "truncate"
                | "script_drop_trigger"
        ) && !profile.supports_mutation()
        {
            return Some(Self::Connection);
        }
        if !mode.allows_note(key) {
            return Some(Self::Mode {
                current: mode,
                required: if AccessMode::ReadWrite.allows_note(key) {
                    AccessMode::ReadWrite
                } else {
                    AccessMode::Admin
                },
                command: label(key),
                key: key.into(),
            });
        }
        None
    }
    pub fn for_sql(
        mode: AccessMode,
        profile: &Profile,
        sql: &str,
        script: bool,
        cx: &App,
    ) -> Option<Self> {
        let key = if script {
            notebook_query::script_command_required(profile.kind, sql)
        } else {
            notebook_query::command_required(profile.kind, sql)
        };
        let Some(key) = key else {
            return Some(Self::Unsupported);
        };
        let global = cx.global::<Preferences>().database.command_enabled(key);
        if !global {
            return Some(Self::Global {
                command: label(key),
            });
        }
        if matches!(
            key,
            "insert"
                | "update"
                | "delete"
                | "create"
                | "alter"
                | "drop"
                | "truncate"
                | "script_drop_trigger"
        ) && !profile.supports_mutation()
        {
            return Some(Self::Connection);
        }
        if !notebook_query::note_mode_allows(mode, profile.kind, sql, script) {
            let required = if notebook_query::note_mode_allows(
                AccessMode::ReadWrite,
                profile.kind,
                sql,
                script,
            ) {
                AccessMode::ReadWrite
            } else if notebook_query::note_mode_allows(AccessMode::Admin, profile.kind, sql, script)
            {
                AccessMode::Admin
            } else {
                return Some(Self::Unsupported);
            };
            return Some(Self::Mode {
                current: mode,
                required,
                command: label(key),
                key: key.into(),
            });
        }
        None
    }
    pub fn blocked_mode(current: AccessMode, required: AccessMode, key: &str) -> Self {
        Self::Mode {
            current,
            required,
            command: label(key),
            key: key.into(),
        }
    }
    pub fn message(&self, cx: &App) -> SharedString {
        match self {
            Self::Mode {
                current,
                required,
                command,
                ..
            } => tr(
                cx,
                "permissions.note_mode",
                &[
                    ("current", &t(cx, current.label())),
                    ("required", &t(cx, required.label())),
                    ("command", command),
                ],
            ),
            Self::Global { command } => tr(cx, "permissions.global", &[("command", command)]),
            Self::Connection => t(cx, "permissions.connection"),
            Self::Unsupported => t(cx, "permissions.unsupported"),
        }
    }
}
