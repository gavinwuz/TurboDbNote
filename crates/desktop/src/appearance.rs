use gpui::App;
use gpui_component::{Theme, ThemeConfig, ThemeRegistry, ThemeSet};
use std::rc::Rc;
pub const CHOICES: [(&str, &str); 4] = [
    ("turbodbnote-celadon", "settings.theme.celadon"),
    ("turbodbnote-mist-blue", "settings.theme.mist_blue"),
    ("turbodbnote-warm-paper", "settings.theme.warm_paper"),
    (
        "turbodbnote-graphite-purple",
        "settings.theme.graphite_purple",
    ),
];
pub fn resolve(name: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
    let set: ThemeSet = serde_json::from_str(include_str!("../assets/light-themes.json"))
        .expect("valid bundled themes");
    set.themes
        .into_iter()
        .find(|t| t.name.as_ref() == name)
        .map(Rc::new)
        .or_else(|| ThemeRegistry::global(cx).themes().get(name).cloned())
}
pub fn initialize(cx: &mut App) {
    if let Some(theme) = resolve(CHOICES[0].0, cx) {
        let dark = Theme::global(cx).mode.is_dark();
        Theme::global_mut(cx).light_theme = theme.clone();
        if !dark {
            Theme::global_mut(cx).apply_config(&theme);
        }
    }
}

pub fn apply_selected(theme: &Rc<ThemeConfig>, cx: &mut App) {
    if Theme::global(cx).mode.is_dark()
        && CHOICES.iter().any(|(name, _)| *name == theme.name.as_ref())
    {
        Theme::global_mut(cx).light_theme = theme.clone();
    } else {
        Theme::global_mut(cx).apply_config(theme);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_light_styles_have_unique_names_and_selection_colors() {
        let set: gpui_component::ThemeSet =
            serde_json::from_str(include_str!("../assets/light-themes.json")).unwrap();
        assert_eq!(set.themes.len(), 4);
        for (theme, (name, _)) in set.themes.iter().zip(super::CHOICES) {
            assert_eq!(theme.name.as_ref(), name);
            assert!(!theme.mode.is_dark());
            let value = serde_json::to_value(theme).unwrap();
            assert!(value["colors"]["foreground"].is_string());
            assert!(value["colors"]["list.active.background"].is_string());
        }
    }
}
