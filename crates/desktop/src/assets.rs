use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

macro_rules! driver_brand {
    ($id:literal) => {
        (
            $id,
            concat!("brands/", $id, ".png"),
            include_bytes!(concat!("../assets/brands/", $id, ".png")),
        )
    };
}
const DRIVER_BRANDS: &[(&str, &str, &[u8])] = &[
    driver_brand!("mysql"),
    driver_brand!("postgresql"),
    driver_brand!("sqlite"),
    driver_brand!("sqlserver"),
    driver_brand!("oracle"),
    driver_brand!("mariadb"),
    driver_brand!("mongodb"),
    driver_brand!("redis"),
    driver_brand!("elasticsearch"),
    driver_brand!("clickhouse"),
    driver_brand!("tidb"),
    driver_brand!("access"),
    driver_brand!("cassandra"),
    driver_brand!("tdengine"),
    driver_brand!("db2"),
    driver_brand!("doris"),
];

pub fn driver_brand_image(id: &str, size: f32) -> gpui::Img {
    use gpui::prelude::*;
    let path = DRIVER_BRANDS
        .iter()
        .find(|(driver, _, _)| *driver == id)
        .map(|(_, path, _)| *path)
        .unwrap_or("icons/database.svg");
    gpui::img(path)
        .size(gpui::px(size))
        .object_fit(gpui::ObjectFit::Contain)
}

/// Extend the component bundle without filesystem lookups at render time.
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, _, bytes)) = DRIVER_BRANDS.iter().find(|(_, name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(*bytes)));
        }
        let bytes: Option<&'static [u8]> = match path {
            "icons/md-bold.svg" => Some(include_bytes!("../assets/icons/md-bold.svg")),
            "icons/md-italic.svg" => Some(include_bytes!("../assets/icons/md-italic.svg")),
            "icons/md-strike.svg" => Some(include_bytes!("../assets/icons/md-strike.svg")),
            "icons/md-code.svg" => Some(include_bytes!("../assets/icons/md-code.svg")),
            "icons/md-code-block.svg" => Some(include_bytes!("../assets/icons/md-code-block.svg")),
            "icons/md-table.svg" => Some(include_bytes!("../assets/icons/md-table.svg")),
            "icons/md-rule.svg" => Some(include_bytes!("../assets/icons/md-rule.svg")),
            "icons/md-image.svg" => Some(include_bytes!("../assets/icons/md-image.svg")),
            "icons/md-quote.svg" => Some(include_bytes!("../assets/icons/md-quote.svg")),
            "icons/md-list.svg" => Some(include_bytes!("../assets/icons/md-list.svg")),
            "icons/md-link.svg" => Some(include_bytes!("../assets/icons/md-link.svg")),

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
            "icons/note-read-mode.svg" => {
                Some(include_bytes!("../assets/icons/note-read-mode.svg"))
            }
            "icons/note-ddl.svg" => Some(include_bytes!("../assets/icons/note-ddl.svg")),
            "icons/ai-permissions.svg" => {
                Some(include_bytes!("../assets/icons/ai-permissions.svg"))
            }
            "icons/note-ai.svg" => Some(include_bytes!("../assets/icons/note-ai.svg")),
            "icons/note-edit-mode.svg" => {
                Some(include_bytes!("../assets/icons/note-edit-mode.svg"))
            }
            "icons/result-grid.svg" => Some(include_bytes!("../assets/icons/result-grid.svg")),
            "icons/result-json.svg" => Some(include_bytes!("../assets/icons/result-json.svg")),
            "icons/note-plan.svg" => Some(include_bytes!("../assets/icons/note-plan.svg")),
            "icons/note-copy-insert.svg" => {
                Some(include_bytes!("../assets/icons/note-copy-insert.svg"))
            }
            "icons/note-columns.svg" => Some(include_bytes!("../assets/icons/note-columns.svg")),
            "icons/note-format.svg" => Some(include_bytes!("../assets/icons/note-format.svg")),
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
                "icons/md-bold.svg",
                "icons/md-italic.svg",
                "icons/md-strike.svg",
                "icons/md-code.svg",
                "icons/md-code-block.svg",
                "icons/md-table.svg",
                "icons/md-rule.svg",
                "icons/md-image.svg",
                "icons/md-quote.svg",
                "icons/md-list.svg",
                "icons/md-link.svg",
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
                "icons/note-read-mode.svg",
                "icons/ai-permissions.svg",
                "icons/note-ai.svg",
                "icons/note-ddl.svg",
                "icons/note-edit-mode.svg",
                "icons/result-grid.svg",
                "icons/result-json.svg",
                "icons/note-plan.svg",
                "icons/note-copy-insert.svg",
                "icons/note-columns.svg",
                "icons/note-format.svg",
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
        items.extend(
            DRIVER_BRANDS
                .iter()
                .filter(|(_, name, _)| name.starts_with(path))
                .map(|(_, name, _)| SharedString::from(*name)),
        );
        Ok(items)
    }
}

pub fn refresh_icon() -> gpui_component::Icon {
    gpui_component::Icon::default().path("icons/refresh.svg")
}

/// Neutral document outline and independently tinted type badge (SVG masks are monochrome).
pub fn note_color(kind: usize, cx: &gpui::App) -> gpui::Hsla {
    use gpui_component::ActiveTheme;
    let (dark, light) = match kind {
        0 => (0x69b8ff, 0x1763a6),
        1 => (0x6bcea0, 0x18784b),
        3 => (0xf87171, 0xb91c1c),
        _ => (0xb79afa, 0x7543b5),
    };
    gpui::rgb(if cx.theme().is_dark() { dark } else { light }).into()
}

pub fn note_icon(kind: usize, cx: &gpui::App) -> gpui::AnyElement {
    let path = match kind {
        0 => "icons/note-sql.svg",
        1 => "icons/note-md.svg",
        _ => "icons/note-file.svg",
    };
    layered_icon("icons/note-frame.svg", path, note_color(kind, cx), cx)
}

pub fn task_icon(cx: &gpui::App) -> gpui::AnyElement {
    layered_icon(
        "icons/task-lines.svg",
        "icons/task-boxes.svg",
        note_color(2, cx),
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
        .size(px(16.))
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

pub fn note_tool(id: impl Into<gpui::ElementId>) -> gpui_component::button::Button {
    use gpui::Styled;
    use gpui_component::{Sizable, button::ButtonVariants};
    gpui_component::button::Button::new(id)
        .with_size(gpui_component::Size::Medium)
        .w(gpui::px(24.))
        .h(gpui::px(24.))
        .ghost()
}
