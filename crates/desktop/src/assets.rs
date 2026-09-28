use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

/// Extend the component bundle without filesystem lookups at render time.
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/task-boxes.svg" => Some(include_bytes!("../assets/icons/task-boxes.svg")),
            "icons/task-lines.svg" => Some(include_bytes!("../assets/icons/task-lines.svg")),
            "icons/note-frame.svg" => Some(include_bytes!("../assets/icons/note-frame.svg")),
            "icons/note-file.svg" => Some(include_bytes!("../assets/icons/note-file.svg")),
            "icons/note-sql.svg" => Some(include_bytes!("../assets/icons/note-sql.svg")),
            "icons/note-md.svg" => Some(include_bytes!("../assets/icons/note-md.svg")),
            "icons/note-save.svg" => Some(include_bytes!("../assets/icons/note-save.svg")),
            "icons/note-run-all.svg" => Some(include_bytes!("../assets/icons/note-run-all.svg")),
            "icons/note-collapse.svg" => Some(include_bytes!("../assets/icons/note-collapse.svg")),
            "icons/note-expand.svg" => Some(include_bytes!("../assets/icons/note-expand.svg")),

            "icons/note-delete.svg" => Some(include_bytes!("../assets/icons/note-delete.svg")),
            "icons/note-run.svg" => Some(include_bytes!("../assets/icons/note-run.svg")),
            "icons/note-insert-before.svg" => {
                Some(include_bytes!("../assets/icons/note-insert-before.svg"))
            }
            "icons/note-insert-after.svg" => {
                Some(include_bytes!("../assets/icons/note-insert-after.svg"))
            }

            "icons/current-time.svg" => Some(include_bytes!("../assets/icons/current-time.svg")),
            "icons/page-first.svg" => Some(include_bytes!("../assets/icons/page-first.svg")),
            "icons/page-last.svg" => Some(include_bytes!("../assets/icons/page-last.svg")),
            "icons/row-count.svg" => Some(include_bytes!("../assets/icons/row-count.svg")),
            "icons/name-generate.svg" => Some(include_bytes!("../assets/icons/name-generate.svg")),
            "icons/project-add.svg" => Some(include_bytes!("../assets/icons/project-add.svg")),
            "icons/group-list.svg" => Some(include_bytes!("../assets/icons/group-list.svg")),
            "icons/refresh.svg" => Some(include_bytes!("../assets/icons/refresh.svg")),
            "icons/focus-cross.svg" => Some(include_bytes!("../assets/icons/focus-cross.svg")),
            "icons/index.svg" => Some(include_bytes!("../assets/icons/index.svg")),
            "icons/key.svg" => Some(include_bytes!("../assets/icons/key.svg")),
            "icons/import.svg" => Some(include_bytes!("../assets/icons/import.svg")),
            "icons/export.svg" => Some(include_bytes!("../assets/icons/export.svg")),

            "icons/db-mysql.svg" => Some(include_bytes!("../assets/icons/db-mysql.svg")),
            "icons/db-postgres.svg" => Some(include_bytes!("../assets/icons/db-postgres.svg")),
            "icons/db-sqlite.svg" => Some(include_bytes!("../assets/icons/db-sqlite.svg")),
            "icons/db-clickhouse.svg" => Some(include_bytes!("../assets/icons/db-clickhouse.svg")),
            "icons/db-redis.svg" => Some(include_bytes!("../assets/icons/db-redis.svg")),
            "icons/data-note.svg" => Some(include_bytes!("../assets/icons/data-note.svg")),
            "icons/database.svg" => Some(include_bytes!("../assets/icons/database.svg")),
            "icons/stop.svg" => Some(include_bytes!("../assets/icons/stop.svg")),
            _ => None,
        };
        match bytes {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_component_assets::Assets.load(path),
        }
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut items = gpui_component_assets::Assets.list(path)?;
        items.extend(
            [
                "icons/task-boxes.svg",
                "icons/task-lines.svg",
                "icons/note-frame.svg",
                "icons/note-file.svg",
                "icons/note-sql.svg",
                "icons/note-md.svg",
                "icons/note-save.svg",
                "icons/note-run-all.svg",
                "icons/note-collapse.svg",
                "icons/note-expand.svg",
                "icons/note-delete.svg",
                "icons/note-run.svg",
                "icons/note-insert-before.svg",
                "icons/note-insert-after.svg",
                "icons/project-add.svg",
                "icons/current-time.svg",
                "icons/page-first.svg",
                "icons/page-last.svg",
                "icons/row-count.svg",
                "icons/name-generate.svg",
                "icons/group-list.svg",
                "icons/refresh.svg",
                "icons/focus-cross.svg",
                "icons/index.svg",
                "icons/key.svg",
                "icons/import.svg",
                "icons/export.svg",
                "icons/db-mysql.svg",
                "icons/db-postgres.svg",
                "icons/db-sqlite.svg",
                "icons/db-clickhouse.svg",
                "icons/db-redis.svg",
                "icons/data-note.svg",
                "icons/database.svg",
                "icons/stop.svg",
            ]
            .into_iter()
            .filter(|name| name.starts_with(path))
            .map(Into::into),
        );
        Ok(items)
    }
}

pub fn refresh_icon() -> gpui_component::Icon {
    gpui_component::Icon::default().path("icons/refresh.svg")
}

/// Neutral document outline and independently tinted type badge (SVG masks are monochrome).
pub fn note_icon(kind: usize, cx: &gpui::App) -> gpui::AnyElement {
    use gpui::rgb;
    use gpui_component::ActiveTheme;
    let (path, color): (&str, gpui::Hsla) = match kind {
        0 => (
            "icons/note-sql.svg",
            if cx.theme().is_dark() {
                rgb(0x69b8ff).into()
            } else {
                rgb(0x1763a6).into()
            },
        ),
        1 => (
            "icons/note-md.svg",
            if cx.theme().is_dark() {
                rgb(0x6bcea0).into()
            } else {
                rgb(0x18784b).into()
            },
        ),
        _ => (
            "icons/note-file.svg",
            if cx.theme().is_dark() {
                rgb(0xb79afa).into()
            } else {
                rgb(0x7543b5).into()
            },
        ),
    };
    layered_icon("icons/note-frame.svg", path, color, cx)
}

pub fn task_icon(cx: &gpui::App) -> gpui::AnyElement {
    use gpui_component::ActiveTheme;
    layered_icon(
        "icons/task-lines.svg",
        "icons/task-boxes.svg",
        cx.theme().primary,
        cx,
    )
}

fn layered_icon(
    base: &'static str,
    badge: &'static str,
    color: gpui::Hsla,
    cx: &gpui::App,
) -> gpui::AnyElement {
    use gpui::{prelude::*, *};
    use gpui_component::ActiveTheme;
    div()
        .relative()
        .size(px(22.))
        .flex_shrink_0()
        .child(
            svg()
                .path(base)
                .size_full()
                .text_color(cx.theme().foreground.opacity(0.65)),
        )
        .child(
            svg()
                .path(badge)
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .text_color(color),
        )
        .into_any_element()
}
