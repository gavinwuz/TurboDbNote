use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

/// Extend the component bundle without filesystem lookups at render time.
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
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
