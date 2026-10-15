//! Shared connection identity for Note, chat and transfer selectors.
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme, StyledExt, menu::PopupMenuItem};
use turbodbn_services::mysql::Profile;
pub fn identity(profile: &Profile) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .size(px(9.))
                .flex_shrink_0()
                .rounded_sm()
                .bg(crate::database::environment_color(&profile.environment)),
        )
        .child(crate::assets::driver_brand_image(
            profile.driver_spec().map(|s| s.id).unwrap_or(""),
            16.,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(profile.name.clone()),
        )
}
pub fn item(profile: &Profile) -> PopupMenuItem {
    let profile = profile.clone();
    PopupMenuItem::element(move |_, cx| {
        let address = if profile.is_file_database() {
            std::path::Path::new(&profile.sqlite_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else if profile.driver_id == "odbc" {
            profile.odbc.dsn.clone()
        } else {
            format!("{}:{}", profile.host, profile.port)
        };
        div()
            .v_flex()
            .w(px(340.))
            .gap_1()
            .child(identity(&profile))
            .child(
                div()
                    .pl(px(29.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(format!(
                        "{} · {} · {}",
                        profile.environment,
                        profile.driver_label(),
                        address
                    )),
            )
    })
}
