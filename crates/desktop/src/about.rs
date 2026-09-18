use crate::{catalog, locale::t};
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme, Icon, IconName, StyledExt, WindowExt, button::Button};
use serde::Deserialize;

#[derive(Deserialize)]
struct Release {
    version: String,
    channel: String,
    released_on: Option<String>,
}

fn metadata() -> Release {
    serde_json::from_str(include_str!("../assets/release.json")).expect("bundled release metadata")
}

fn row(icon: IconName, label: &str, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .h_flex()
        .gap_3()
        .py_2()
        .child(Icon::new(icon).text_color(cx.theme().muted_foreground))
        .child(
            div()
                .w(px(124.))
                .text_color(cx.theme().muted_foreground)
                .child(t(cx, label)),
        )
        .child(div().flex_1().child(value.into()))
}

pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, cx| {
        let release = metadata();
        let date = release
            .released_on
            .map(SharedString::from)
            .unwrap_or_else(|| t(cx, "about.release.unpublished"));
        let channel = t(
            cx,
            if release.channel == "stable" {
                "about.channel.stable"
            } else {
                "about.channel.preview"
            },
        );
        dialog.title(t(cx, "about.title")).w(px(480.)).child(
            div()
                .v_flex()
                .gap_4()
                .child(
                    div()
                        .h_flex()
                        .gap_3()
                        .child(Icon::default().path("icons/data-note.svg").size(px(36.)))
                        .child(
                            div()
                                .v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(rems(crate::typography::BRAND))
                                        .child("TurboDbNote"),
                                )
                                .child(
                                    div()
                                        .text_size(rems(crate::typography::BODY))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t(cx, "about.description")),
                                ),
                        ),
                )
                .child(
                    div()
                        .v_flex()
                        .p_3()
                        .rounded_lg()
                        .bg(cx.theme().muted)
                        .child(row(
                            IconName::Info,
                            "about.version.label",
                            format!("v{}", release.version),
                            cx,
                        ))
                        .child(row(
                            IconName::Calendar,
                            "about.release_date.label",
                            date,
                            cx,
                        ))
                        .child(row(
                            IconName::GalleryVerticalEnd,
                            "about.channel.label",
                            channel,
                            cx,
                        ))
                        .child(row(
                            IconName::SquareTerminal,
                            "about.platform.label",
                            format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
                            cx,
                        ))
                        .child(row(
                            IconName::BookOpen,
                            "about.license.label",
                            env!("CARGO_PKG_LICENSE"),
                            cx,
                        )),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("about-github")
                                .icon(IconName::GitHub)
                                .label("GitHub")
                                .on_click(|_, _, cx| cx.open_url(catalog::REPOSITORY)),
                        )
                        .child(
                            Button::new("about-releases")
                                .icon(IconName::ExternalLink)
                                .label(t(cx, "about.action.releases"))
                                .on_click(|_, _, cx| cx.open_url(catalog::RELEASES)),
                        ),
                ),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::metadata;
    #[test]
    fn release_metadata_matches_package() {
        let release = metadata();
        assert_eq!(release.version, env!("CARGO_PKG_VERSION"));
        assert!(matches!(release.channel.as_str(), "preview" | "stable"));
        if let Some(date) = release.released_on {
            assert_eq!(date.len(), 10);
            assert_eq!(&date[4..5], "-");
            assert_eq!(&date[7..8], "-");
        } else {
            assert_eq!(release.channel, "preview");
        }
    }
}
