use crate::preferences::Preferences;
use gpui::{App, Global, SharedString};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
pub struct LanguagePack {
    pub id: String,
    pub name: String,
    pub strings: BTreeMap<String, String>,
}
pub struct Locale {
    pub packs: Vec<LanguagePack>,
    pub active: String,
}
impl Global for Locale {}
impl Locale {
    pub fn load(active: String) -> Self {
        gpui_component::set_locale(&active);
        let mut packs: Vec<LanguagePack> = [
            include_str!("../assets/locales/zh-CN.json"),
            include_str!("../assets/locales/en.json"),
        ]
        .iter()
        .map(|json| serde_json::from_str(json).expect("bundled language pack"))
        .collect();
        if let Ok(path) = Preferences::path()
            && let Some(parent) = path.parent()
            && let Ok(entries) = std::fs::read_dir(parent.join("locales"))
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                if entry.metadata().is_ok_and(|m| m.len() > 1_048_576) {
                    continue;
                }
                if let Ok(data) = std::fs::read_to_string(path)
                    && let Ok(pack) = serde_json::from_str::<LanguagePack>(&data)
                    && !pack.id.is_empty()
                    && !pack.name.is_empty()
                {
                    if let Some(existing) = packs.iter_mut().find(|p| p.id == pack.id) {
                        existing.name = pack.name;
                        existing.strings.extend(pack.strings);
                    } else {
                        packs.push(pack);
                    }
                }
            }
        }
        Self { packs, active }
    }
}
pub fn t(cx: &App, key: &str) -> SharedString {
    cx.try_global::<Locale>()
        .and_then(|locale| locale.packs.iter().find(|p| p.id == locale.active))
        .and_then(|pack| pack.strings.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_owned())
        .into()
}

pub fn apply_font(cx: &mut App) {
    let general = cx.global::<Preferences>().general.clone();
    gpui_component::Theme::global_mut(cx).font_family = general.font_family.into();
    gpui_component::Theme::global_mut(cx).font_size = gpui::px(general.font_size.clamp(12., 24.));
    gpui_component::Theme::global_mut(cx).mono_font_size =
        gpui::px(general.font_size.clamp(12., 24.));
}
