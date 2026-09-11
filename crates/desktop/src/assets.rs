use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

/// Extend the component bundle without filesystem lookups at render time.
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/data-note.svg" => Some(include_bytes!("../assets/icons/data-note.svg")),
            "icons/database.svg" => Some(include_bytes!("../assets/icons/database.svg")),
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
            ["icons/data-note.svg", "icons/database.svg"]
                .into_iter()
                .filter(|name| name.starts_with(path))
                .map(Into::into),
        );
        Ok(items)
    }
}
