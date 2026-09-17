#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod about;
mod assets;
mod auto_connect;
mod catalog;
mod cell;
mod chat;
mod chat_history;
mod configuration;
mod locale;
mod panels;
mod preferences;
mod settings;
mod tabs;
mod typography;
#[cfg(target_os = "windows")]
mod windows_icon;
mod workspace;

use gpui::*;
use gpui_component::{Root, TitleBar};

fn main() {
    #[cfg(target_os = "windows")]
    let _install_guard =
        windows_icon::install_guard().expect("Unable to register running application");
    Application::new().with_assets(assets::Assets).run(|cx| {
        gpui_component::init(cx);
        let preferences = preferences::Preferences::load();
        if let Some(dark) = preferences.dark {
            gpui_component::Theme::change(
                if dark {
                    gpui_component::ThemeMode::Dark
                } else {
                    gpui_component::ThemeMode::Light
                },
                None,
                cx,
            );
        }
        gpui_component::Theme::global_mut(cx).font_family =
            preferences.general.font_family.clone().into();
        gpui_component::Theme::global_mut(cx).font_size =
            px(preferences.general.font_size.clamp(12., 24.));
        cx.set_global(locale::Locale::load(preferences.general.language.clone()));
        let events = cx.new(|_| tabs::TabEvents);
        cx.set_global(tabs::TabRouter(events));
        cx.set_global(preferences);
        crate::locale::apply_font(cx);
        // Theme reloads can reset typography. User font settings take precedence.
        cx.observe_global::<gpui_component::Theme>(|cx| {
            let general = &cx.global::<preferences::Preferences>().general;
            let theme = cx.global::<gpui_component::Theme>();
            let size = px(general.font_size.clamp(12., 24.));
            if theme.font_size != size
                || theme.mono_font_size != size
                || theme.font_family.as_ref() != general.font_family
            {
                crate::locale::apply_font(cx);
                cx.refresh_windows();
            }
        })
        .detach();

        if let Ok(path) = preferences::Preferences::path()
            && let Some(parent) = path.parent()
        {
            let _ = gpui_component::ThemeRegistry::watch_dir(parent.join("themes"), cx, |cx| {
                if let Some(name) = cx
                    .global::<preferences::Preferences>()
                    .general
                    .theme
                    .clone()
                    && let Some(theme) = gpui_component::ThemeRegistry::global(cx)
                        .themes()
                        .get(name.as_str())
                        .cloned()
                {
                    gpui_component::Theme::global_mut(cx).apply_config(&theme);
                }
                crate::locale::apply_font(cx);
                cx.refresh_windows();
            });
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1440.), px(960.)),
                cx,
            ))),
            window_min_size: Some(size(px(1000.), px(700.))),
            titlebar: Some(TitlebarOptions {
                title: Some("TurboDbNote".into()),
                ..TitleBar::title_bar_options()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            #[cfg(target_os = "windows")]
            if let Err(error) = windows_icon::install(window) {
                eprintln!("Unable to set TurboDbNote window icons: {error}");
            }
            let view = cx.new(|cx| workspace::Workspace::new(window, cx));
            let weak = view.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |view, cx| view.prepare_close(window, cx))
                    .unwrap_or(true)
            });
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("Unable to open TurboDbNote window");
        cx.activate(true);
    });
}
