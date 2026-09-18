use crate::{
    locale_catalog::{self, LanguagePack, Strings},
    preferences::Preferences,
};
use gpui::{App, Global, SharedString};
use std::{path::Path, sync::OnceLock};

fn builtins() -> &'static Vec<LanguagePack> {
    static PACKS: OnceLock<Vec<LanguagePack>> = OnceLock::new();
    PACKS.get_or_init(|| {
        [
            include_str!("../assets/locales/en.json"),
            include_str!("../assets/locales/zh-CN.json"),
        ]
        .iter()
        .map(|json| locale_catalog::parse_pack(json).expect("validated bundled pack"))
        .collect()
    })
}
fn aliases() -> &'static Strings {
    static ALIASES: OnceLock<Strings> = OnceLock::new();
    ALIASES.get_or_init(|| {
        locale_catalog::parse_aliases(include_str!("../assets/locales/legacy-keys.json"))
            .expect("validated aliases")
    })
}
fn english() -> &'static LanguagePack {
    &builtins()[0]
}
fn canonical(key: &str) -> &str {
    aliases().get(key).map(String::as_str).unwrap_or(key)
}
pub struct Locale {
    pub packs: Vec<LanguagePack>,
    pub active: String,
}
impl Global for Locale {}
impl Locale {
    pub fn load(active: String) -> Self {
        let directory = Preferences::path()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("locales")));
        let locale = Self::load_from(active, directory.as_deref());
        gpui_component::set_locale(&locale.active);
        locale
    }
    fn load_from(active: String, directory: Option<&Path>) -> Self {
        let mut packs = builtins().clone();
        debug_assert!(locale_catalog::validate_catalog(&packs[0], &packs[1], aliases()).is_ok());
        if let Some(directory) = directory
            && let Ok(entries) = std::fs::read_dir(directory)
        {
            let mut paths = entries
                .flatten()
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                if path.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                let result = (|| -> Result<LanguagePack, String> {
                    use std::io::Read;
                    let mut data = String::new();
                    std::fs::File::open(&path)
                        .map_err(|_| "cannot open pack")?
                        .take(1_048_577)
                        .read_to_string(&mut data)
                        .map_err(|_| "cannot read pack")?;
                    if data.len() > 1_048_576 {
                        return Err("pack exceeds 1 MiB".into());
                    }
                    locale_catalog::normalize_external(
                        locale_catalog::parse_pack(&data)?,
                        english(),
                        aliases(),
                    )
                })();
                match result {
                    Ok(pack) => {
                        if let Some(existing) = packs.iter_mut().find(|p| p.id == pack.id) {
                            existing.name = pack.name;
                            existing.strings.extend(pack.strings);
                        } else {
                            packs.push(pack);
                        }
                    }
                    Err(error) => eprintln!("Ignored invalid language pack: {error}"),
                }
            }
        }
        Self { packs, active }
    }
    fn resolve(&self, key: &str) -> Option<&str> {
        let key = canonical(key);
        self.packs
            .iter()
            .find(|p| p.id == self.active)
            .and_then(|p| p.strings.get(key))
            .or_else(|| english().strings.get(key))
            .map(String::as_str)
    }
}
fn missing(key: &str) -> SharedString {
    #[cfg(debug_assertions)]
    eprintln!("Missing translation key or arguments: {key}");
    #[cfg(not(debug_assertions))]
    let _ = key;
    english().strings["common.translation_missing"]
        .clone()
        .into()
}
pub fn t(cx: &App, key: &str) -> SharedString {
    let text = cx
        .try_global::<Locale>()
        .and_then(|locale| locale.resolve(key))
        .or_else(|| english().strings.get(canonical(key)).map(String::as_str));
    match text {
        Some(text) => text.to_owned().into(),
        None => missing(key),
    }
}
pub fn tr(cx: &App, key: &str, args: &[(&str, &str)]) -> SharedString {
    let template = t(cx, key);
    locale_catalog::format_message(template.as_ref(), args)
        .map(Into::into)
        .unwrap_or_else(|_| missing(key))
}
/// For internally generated semantic status keys or unmodified external text.
/// Raw backend errors are never treated as legacy translation keys.
pub fn message(cx: &App, value: &str) -> SharedString {
    if english().strings.contains_key(value) {
        t(cx, value)
    } else {
        value.to_owned().into()
    }
}
pub fn apply_font(cx: &mut App) {
    let general = cx.global::<Preferences>().general.clone();
    gpui_component::Theme::global_mut(cx).font_family = general.font_family.into();
    gpui_component::Theme::global_mut(cx).font_size = gpui::px(general.font_size.clamp(12., 24.));
    gpui_component::Theme::global_mut(cx).mono_font_size =
        gpui::px(general.font_size.clamp(12., 24.));
}
#[cfg(test)]
mod tests {
    use super::Locale;
    #[test]
    fn external_overrides_builtin_then_falls_back_to_english() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("zh.json"),
            r#"{"id":"zh-CN","name":"中文","strings":{"检查器":"自定义检查器"}}"#,
        )
        .unwrap();
        let locale = Locale::load_from("zh-CN".into(), Some(dir.path()));
        assert_eq!(
            locale.resolve("panel.inspector.title"),
            Some("自定义检查器")
        );
        assert_eq!(locale.resolve("panel.output.title"), Some("输出"));
        std::fs::write(dir.path().join("fr.json"),r#"{"schema_version":2,"id":"fr","name":"Français","strings":{"panel.output.title":"Sortie"}}"#).unwrap();
        let locale = Locale::load_from("fr".into(), Some(dir.path()));
        assert_eq!(locale.resolve("panel.output.title"), Some("Sortie"));
        assert_eq!(locale.resolve("panel.inspector.title"), Some("Inspector"));
        assert_eq!(
            Locale::load_from("missing".into(), None).resolve("panel.output.title"),
            Some("Output")
        );
    }
    #[test]
    fn malformed_external_pack_does_not_break_builtin_language() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.json"),r#"{"id":"en","name":"English","strings":{"panel.output.title":"A","panel.output.title":"B"}}"#).unwrap();
        assert_eq!(
            Locale::load_from("en".into(), Some(dir.path())).resolve("panel.output.title"),
            Some("Output")
        );
    }
}
