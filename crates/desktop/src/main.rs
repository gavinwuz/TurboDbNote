#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod cell;
mod panels;
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
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("Unable to open TurboDbNote window");
        cx.activate(true);
    });
}
