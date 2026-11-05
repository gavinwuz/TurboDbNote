//! Shared connection identity for Note, chat and transfer selectors.
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme, StyledExt, menu::PopupMenuItem};
use turbodbn_services::mysql::Profile;
/// Display identity only: never includes credentials beyond the explicit username.
pub(super) fn address(profile: &Profile, compact: bool) -> String {
    if profile.is_file_database() {
        return std::path::Path::new(&profile.sqlite_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    if profile.driver_id == "odbc" {
        return profile.odbc.dsn.clone();
    }
    let host = profile.host.trim();
    let host = match host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
    {
        Ok(std::net::IpAddr::V4(ip)) if compact => {
            let octets = ip.octets();
            format!("{}.{}", octets[2], octets[3])
        }
        Ok(std::net::IpAddr::V6(ip)) => format!("[{ip}]"),
        _ => host.to_owned(),
    };
    let user = profile.user.trim();
    if user.is_empty() {
        format!("{host}:{}", profile.port)
    } else {
        format!("{user}@{host}:{}", profile.port)
    }
}
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
        let address = address(&profile, false);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn shortened_and_full_identities_share_account_and_port() {
        let mut profile = Profile {
            user: "admin".into(),
            host: "192.168.140.17".into(),
            port: 3306,
            ..Default::default()
        };
        assert_eq!(address(&profile, true), "admin@140.17:3306");
        assert_eq!(address(&profile, false), "admin@192.168.140.17:3306");
        profile.host = "db.prod.example.com".into();
        assert_eq!(address(&profile, true), "admin@db.prod.example.com:3306");
        profile.host = "2001:db8::17".into();
        assert_eq!(address(&profile, true), "admin@[2001:db8::17]:3306");
        profile.user.clear();
        assert_eq!(address(&profile, false), "[2001:db8::17]:3306");
    }
}
