#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(test)]
#[path = "../../../vendor/gpui-component/src/input/geometry_index.rs"]
mod input_geometry_contracts;
#[cfg(test)]
#[path = "../../../vendor/gpui-component/src/input/highlight_map.rs"]
mod input_highlight_contracts;

mod content_scroll;
mod ddl_picker;
mod grid_metrics;
mod index_workbench;
mod object_browser;
mod table_compare;
mod table_designer;

mod about;
mod app_store;
mod assets;
mod auto_connect;
mod catalog;
mod catalog_cache;
mod cell;
mod chat;
mod chat_history;
mod configuration;
mod connection_choice;
mod connection_secret;
mod connection_store;
mod context_menu;
mod database;
mod diagnostics;
mod driver_components;
mod drivers;
mod duration;
mod feedback;
mod locale;
mod locale_catalog;
mod mongo_tree;
mod note_library;
mod note_permissions;
mod notebook;
mod operation_log;
mod panels;
mod preferences;
mod query_grid;
mod result_document;
mod script_authorize;
mod script_cell;
mod script_import;
mod script_results;
mod settings;
mod table_view;
mod tabs;
mod task_list;
use turbodbn_services::database::transfer;
mod typography;
mod updates;
#[cfg(target_os = "windows")]
mod windows_icon;
#[cfg(target_os = "windows")]
mod windows_tray;
mod workspace;

use gpui::*;
use gpui_component::{Root, TitleBar};

fn main() {
    #[cfg(target_os = "windows")]
    let _install_guard =
        windows_icon::install_guard().expect("Unable to register running application");
    Application::new().with_assets(assets::Assets).run(|cx| {
        gpui_component::init(cx);
        cell::markdown_tools::init(cx);
        register_sql_highlighting();
        let diagnostics = diagnostics::start();
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
        cx.set_global(tabs::TabRouter(events, 1, vec![]));
        let _ =
            turbodbn_services::timeout_policy::configure(preferences.database.timeout_defaults());
        cx.set_global(preferences);
        cx.observe_global::<preferences::Preferences>(|cx| {
            let _ = turbodbn_services::timeout_policy::configure(
                cx.global::<preferences::Preferences>()
                    .database
                    .timeout_defaults(),
            );
        })
        .detach();
        feedback::init(cx);
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
        cx.on_window_closed(move |cx| {
            let _keep_alive = &diagnostics;
            if cx.windows().is_empty() {
                turbodbn_diagnostics::shutdown();
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
                title: Some("TurboDBNote".into()),
                ..TitleBar::title_bar_options()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            #[cfg(target_os = "windows")]
            if let Err(error) = windows_icon::install(window) {
                eprintln!("Unable to set TurboDBNote window icons: {error}");
            }
            let view = cx.new(|cx| workspace::Workspace::new(window, cx));
            if std::env::args_os().any(|arg| arg == "--update-completed") {
                window.defer(cx, |_, cx| {
                    feedback::notify(
                        gpui_component::notification::NotificationType::Success,
                        locale::tr(
                            cx,
                            "updates.completed",
                            &[("version", env!("CARGO_PKG_VERSION"))],
                        ),
                        cx,
                    );
                });
            }

            let weak = view.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |view, cx| view.request_window_close(window, cx))
                    .unwrap_or(true)
            });
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("Unable to open TurboDBNote window");
        cx.activate(true);
    });
}

unsafe extern "C" {
    fn tree_sitter_mysql_comments() -> *const ();
}
pub(crate) const SQL_LIGHT_LANGUAGE: &str = "sql-light";

fn register_sql_highlighting() {
    use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};
    LanguageRegistry::singleton().register(
        "sql",
        &LanguageConfig::new(
            "sql",
            tree_sitter_sequel::LANGUAGE.into(),
            vec![],
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
    let language =
        unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_mysql_comments) };
    // Reuse the flat comment-wrapper grammar with no injections or style queries.
    // Component's built-in `text` language still runs a JSON parser on every edit.
    LanguageRegistry::singleton().register(
        SQL_LIGHT_LANGUAGE,
        &LanguageConfig::new(SQL_LIGHT_LANGUAGE, language.into(), vec![], "", "", ""),
    );
    LanguageRegistry::singleton().register(
        "mysql",
        &LanguageConfig::new(
            "mysql",
            language.into(),
            vec!["sql".into()],
            "(comment) @comment",
            r#"((sql) @injection.content (#set! injection.language "sql"))"#,
            "",
        ),
    );
}

#[cfg(test)]
mod sql_highlighting_tests {
    #[test]
    fn light_sql_has_no_per_token_style_runs_and_preserves_the_document() {
        use gpui_component::{
            Rope,
            highlighter::{HighlightTheme, SyntaxHighlighter},
        };
        super::register_sql_highlighting();
        let sql = format!(
            "SELECT id FROM t WHERE id IN ({}); -- comment\nSELECT '#text';",
            (0..5000)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let rope = Rope::from(sql.as_str());
        let mut highlighter = SyntaxHighlighter::new(super::SQL_LIGHT_LANGUAGE);
        highlighter.update(None, &rope);
        assert!(
            highlighter
                .styles(&(0..sql.len()), &HighlightTheme::default_dark())
                .len()
                <= 1
        );
        assert_eq!(rope.to_string(), sql);
    }

    #[test]
    fn mysql_hash_comments_match_dash_without_recoloring_strings() {
        use gpui_component::{
            Rope,
            highlighter::{HighlightTheme, SyntaxHighlighter},
        };
        super::register_sql_highlighting();
        let sql = "SHOW CREATE TABLE `seach_works`;\n# SELECT 不执行\n-- FROM 不执行\nSELECT '#字符串', '--字符串', `hash#column`;\n/* # block */\nSELECT COUNT(*) FROM t;";
        let mut highlighter = SyntaxHighlighter::new("mysql");
        highlighter.update(None, &Rope::from(sql));
        for theme in [
            HighlightTheme::default_dark(),
            HighlightTheme::default_light(),
        ] {
            let styles = highlighter.styles(&(0..sql.len()), &theme);
            for text in ["# SELECT 不执行", "-- FROM 不执行"] {
                let start = sql.find(text).unwrap();
                assert!(
                    styles.iter().any(|(r, s)| r.start <= start
                        && r.end >= start + text.len()
                        && s.color == theme.style("comment").unwrap().color),
                    "{text}"
                );
            }
            let start = sql.find("'#字符串'").unwrap();
            assert!(styles.iter().any(|(r, s)| r.start <= start
                && r.end >= start + "'#字符串'".len()
                && s.color == theme.style("string").unwrap().color));
            let start = sql.rfind("SELECT").unwrap();
            assert!(styles.iter().any(|(r, s)| r.start <= start
                && r.end >= start + 6
                && s.color == theme.style("keyword").unwrap().color));
        }
    }
    #[test]
    fn sql_keywords_functions_and_strings_receive_theme_styles() {
        use gpui_component::{
            Rope,
            highlighter::{HighlightTheme, SyntaxHighlighter},
        };
        super::register_sql_highlighting();
        let sql = "SELECT COUNT(u.id) FROM users u JOIN other_users o ON u.id=o.id WHERE u.name = 'SELECT' AND u.id > 1 OR u.id = 0 UNION SELECT id FROM other_users; -- FROM";
        let mut highlighter = SyntaxHighlighter::new("sql");
        highlighter.update(None, &Rope::from(sql));
        for theme in [
            HighlightTheme::default_dark(),
            HighlightTheme::default_light(),
        ] {
            let styles = highlighter.styles(&(0..sql.len()), &theme);
            for word in ["SELECT", "FROM", "WHERE", "AND", "OR", "JOIN", "UNION"] {
                let offset = sql.find(word).unwrap();
                assert!(
                    styles.iter().any(|(range, style)| range.start <= offset
                        && range.end >= offset + word.len()
                        && style.color == theme.style("keyword").unwrap().color),
                    "missing keyword: {word}"
                );
            }
            let offset = sql.find("COUNT").unwrap();
            assert!(styles.iter().any(|(range, style)| range.start <= offset
                && range.end >= offset + 5
                && style.color == theme.style("function").unwrap().color));
            let comment = sql.find("-- FROM").unwrap();
            assert!(styles.iter().any(|(range, style)| range.start <= comment
                && range.end >= comment + 7
                && style.color == theme.style("comment").unwrap().color));
            let offset = sql.find("'SELECT'").unwrap();
            assert!(styles.iter().any(|(range, style)| range.start <= offset
                && range.end >= offset + 8
                && style.color == theme.style("string").unwrap().color));
        }
    }
}
