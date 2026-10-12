use crate::{cell::CellView, locale::t};
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Icon, IconName, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    input::RopeExt,
    menu::{DropdownMenu, PopupMenuItem},
};
use std::ops::Range;

actions!(markdown_tools, [Bold, Italic, Link]);
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-b", Bold, Some("NoteMarkdown > Input")),
        KeyBinding::new("ctrl-i", Italic, Some("NoteMarkdown > Input")),
        KeyBinding::new("ctrl-k", Link, Some("NoteMarkdown > Input")),
    ]);
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Heading(u8),
    Bold,
    Italic,
    Strike,
    Code,
    Bullets,
    Numbers,
    Tasks,
    Link,
    Quote,
    CodeBlock,
    Table,
    Rule,
    Image,
}
pub(super) struct FormatMemo {
    format: Format,
    after: SharedString,
    caret: usize,
}
struct Edit {
    range: Range<usize>,
    text: String,
    caret: usize,
    select_ascii: usize,
}
fn byte_offset(source: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in source.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += ch.len_utf16();
    }
    source.len()
}
fn wrap(source: &str, range: Range<usize>, marker: &str) -> Edit {
    let selected = &source[range.clone()];
    let exact_before = source[..range.start].ends_with(marker)
        && !source[..range.start - marker.len()].ends_with(&marker[..1]);
    let exact_after = source[range.end..].starts_with(marker)
        && !source[range.end + marker.len()..].starts_with(&marker[..1]);
    if exact_before && exact_after {
        let start = range.start - marker.len();
        return Edit {
            range: start..range.end + marker.len(),
            text: selected.into(),
            caret: start + selected.len(),
            select_ascii: 0,
        };
    }
    if selected.len() >= marker.len() * 2
        && selected.starts_with(marker)
        && selected.ends_with(marker)
    {
        let text = selected[marker.len()..selected.len() - marker.len()].to_string();
        let caret = range.start + text.len();
        return Edit {
            range,
            text,
            caret,
            select_ascii: 0,
        };
    }
    Edit {
        caret: range.start + marker.len() + selected.len(),
        range,
        text: format!("{marker}{selected}{marker}"),
        select_ascii: 0,
    }
}
fn list_body(line: &str) -> (&str, Option<Format>) {
    if let Some(text) = line
        .strip_prefix("- [ ] ")
        .or_else(|| line.strip_prefix("- [x] "))
    {
        return (text, Some(Format::Tasks));
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(marker) {
            return (text, Some(Format::Bullets));
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && line[digits..].starts_with(". ") {
        return (&line[digits + 2..], Some(Format::Numbers));
    }
    if let Some(text) = line.strip_prefix("> ") {
        return (text, Some(Format::Quote));
    }
    (line, None)
}
fn transform(source: &str, range: Range<usize>, format: Format) -> Edit {
    match format {
        Format::Bold => wrap(source, range, "**"),
        Format::Italic => wrap(source, range, "*"),
        Format::Strike => wrap(source, range, "~~"),
        Format::Code => {
            let text = &source[range.clone()];
            let length = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0) + 1;
            let marker = "`".repeat(length);
            if text.starts_with('`') || text.ends_with('`') {
                Edit {
                    caret: range.start + marker.len() + 1 + text.len(),
                    range,
                    text: format!("{marker} {text} {marker}"),
                    select_ascii: 0,
                }
            } else {
                wrap(source, range, &marker)
            }
        }
        Format::Heading(_) | Format::Bullets | Format::Numbers | Format::Tasks | Format::Quote => {
            let start = source[..range.start].rfind('\n').map_or(0, |at| at + 1);
            let selected_end = if range.is_empty() {
                range.end
            } else {
                source[..range.end]
                    .char_indices()
                    .last()
                    .map_or(0, |(offset, _)| offset)
            };
            let end = source[selected_end..]
                .find('\n')
                .map_or(source.len(), |at| selected_end + at);
            let block = &source[start..end];
            let toggle = !matches!(format, Format::Heading(_))
                && block
                    .split('\n')
                    .all(|line| list_body(line.trim_end_matches('\r')).1 == Some(format));
            let text = block
                .split('\n')
                .enumerate()
                .map(|(index, line)| {
                    let cr = line.ends_with('\r');
                    let raw = line.trim_end_matches('\r');
                    let result = if let Format::Heading(level) = format {
                        let hashes = raw.bytes().take_while(|ch| *ch == b'#').count();
                        let body = if (1..=6).contains(&hashes) && raw[hashes..].starts_with(' ') {
                            &raw[hashes + 1..]
                        } else {
                            raw
                        };
                        if level == 0 {
                            body.to_string()
                        } else {
                            format!("{} {body}", "#".repeat(level as usize))
                        }
                    } else {
                        let body = list_body(raw).0;
                        if toggle {
                            body.to_string()
                        } else {
                            match format {
                                Format::Bullets => format!("- {body}"),
                                Format::Numbers => format!("{}. {body}", index + 1),
                                Format::Tasks => format!("- [ ] {body}"),
                                _ => format!("> {body}"),
                            }
                        }
                    };
                    format!("{result}{}", if cr { "\r" } else { "" })
                })
                .collect::<Vec<_>>()
                .join("\n");
            Edit {
                range: start..end,
                caret: start + text.len(),
                text,
                select_ascii: 0,
            }
        }
        Format::Link | Format::Image => {
            let selected = &source[range.clone()];
            let name = if selected.is_empty() {
                "text"
            } else {
                selected
            };
            let prefix = if format == Format::Image { "!" } else { "" };
            let name = name
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]");
            let url = if format == Format::Image {
                "image-url"
            } else {
                "https://example.com"
            };
            let text = format!("{prefix}[{name}]({url})");
            Edit {
                caret: range.start + text.len() - url.len() - 1,
                range,
                text,
                select_ascii: url.len(),
            }
        }
        _ => {
            let selected = &source[range.clone()];
            let (text, inside) = match format {
                Format::CodeBlock => {
                    let size = selected
                        .split(|ch| ch != '`')
                        .map(str::len)
                        .max()
                        .unwrap_or(0)
                        .max(2)
                        + 1;
                    let fence = "`".repeat(size);
                    (format!("{fence}\n{selected}\n{fence}"), size + 1)
                }
                Format::Table => (
                    format!(
                        "| A | B |\n| --- | --- |\n| {} |  |",
                        selected
                            .replace('|', "\\|")
                            .replace("\r\n", "<br>")
                            .replace('\n', "<br>")
                    ),
                    2,
                ),
                _ => (
                    if selected.is_empty() {
                        "---".into()
                    } else {
                        format!("---\n{selected}")
                    },
                    3,
                ),
            };
            let leading = usize::from(range.start > 0 && !source[..range.start].ends_with('\n'));
            let trailing = range.end < source.len() && !source[range.end..].starts_with('\n');
            let replacement = format!(
                "{}{text}{}",
                if leading > 0 { "\n" } else { "" },
                if trailing { "\n" } else { "" }
            );
            Edit {
                caret: range.start + leading + inside,
                range,
                text: replacement,
                select_ascii: 0,
            }
        }
    }
}
impl CellView {
    pub(super) fn markdown_format(
        &mut self,
        format: Format,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editing
            || self.batch_locked
            || !matches!(
                self.cell.content,
                turbodbn_core::CellContent::Markdown { .. }
            )
        {
            return;
        }
        let toggle = matches!(
            format,
            Format::Bold
                | Format::Italic
                | Format::Strike
                | Format::Code
                | Format::Bullets
                | Format::Numbers
                | Format::Tasks
                | Format::Quote
        );
        if toggle
            && self.markdown_last.as_ref().is_some_and(|memo| {
                memo.format == format
                    && memo.after == self.editor.read(cx).value()
                    && memo.caret == self.editor.read(cx).cursor()
            })
        {
            self.markdown_last = None;
            let editor = self.editor.clone();
            window.defer(cx, move |window, cx| {
                editor.update(cx, |input, cx| input.focus(window, cx));
                window.dispatch_action(Box::new(gpui_component::input::Undo), cx);
            });
            return;
        }
        let count = self.editor.update(cx, |input, cx| {
            let source = input.value();
            let selection = input
                .selected_text_range(false, window, cx)
                .map(|selection| selection.range)
                .unwrap_or(0..0);
            let edit = transform(
                &source,
                byte_offset(&source, selection.start)..byte_offset(&source, selection.end),
                format,
            );
            let range = source[..edit.range.start].encode_utf16().count()
                ..source[..edit.range.end].encode_utf16().count();
            input.replace_text_in_range(Some(range), &edit.text, window, cx);
            let position = input.text().offset_to_position(edit.caret);
            input.set_cursor_position(position, window, cx);
            edit.select_ascii
        });
        self.markdown_last = Some(FormatMemo {
            format,
            after: self.editor.read(cx).value(),
            caret: self.editor.read(cx).cursor(),
        });
        let editor = self.editor.clone();
        window.defer(cx, move |window, cx| {
            editor.update(cx, |input, cx| input.focus(window, cx));
            for _ in 0..count {
                if let Ok(action) = cx.build_action("ui::SelectRight", None) {
                    window.dispatch_action(action, cx);
                }
            }
        });
    }
    pub(super) fn markdown_toolbar(&self, cx: &Context<Self>) -> Div {
        let owner = cx.entity().downgrade();
        let headings = owner.clone();
        let lists = owner;
        let mut bar = div().h_flex().flex_wrap().gap_1().items_center().child(
            Button::new("md-heading")
                .small()
                .ghost()
                .icon(IconName::CaseSensitive)
                .tooltip(t(cx, "md.heading"))
                .dropdown_caret(true)
                .disabled(self.batch_locked)
                .dropdown_menu(move |mut menu, _, cx| {
                    for (level, key) in [
                        (0, "md.paragraph"),
                        (1, "md.h1"),
                        (2, "md.h2"),
                        (3, "md.h3"),
                        (4, "md.h4"),
                        (5, "md.h5"),
                        (6, "md.h6"),
                    ] {
                        let owner = headings.clone();
                        menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.markdown_format(Format::Heading(level), window, cx)
                                });
                            },
                        ));
                    }
                    menu
                }),
        );
        for (format, key, path) in [
            (Format::Bold, "md.bold", "icons/md-bold.svg"),
            (Format::Italic, "md.italic", "icons/md-italic.svg"),
            (Format::Strike, "md.strike", "icons/md-strike.svg"),
            (Format::Code, "md.code", "icons/md-code.svg"),
        ] {
            bar = bar.child(
                Button::new(SharedString::from(key))
                    .small()
                    .ghost()
                    .icon(Icon::default().path(path))
                    .tooltip(t(cx, key))
                    .disabled(self.batch_locked)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.markdown_format(format, window, cx)
                    })),
            );
        }
        bar = bar.child(
            Button::new("md-list")
                .small()
                .ghost()
                .icon(Icon::default().path("icons/md-list.svg"))
                .tooltip(t(cx, "md.list"))
                .dropdown_caret(true)
                .disabled(self.batch_locked)
                .dropdown_menu(move |mut menu, _, cx| {
                    for (format, key) in [
                        (Format::Bullets, "md.bullets"),
                        (Format::Numbers, "md.numbers"),
                        (Format::Tasks, "md.tasks"),
                    ] {
                        let owner = lists.clone();
                        menu = menu.item(
                            PopupMenuItem::new(t(cx, key))
                                .icon(IconName::Menu)
                                .on_click(move |_, window, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.markdown_format(format, window, cx)
                                    });
                                }),
                        );
                    }
                    menu
                }),
        );
        for (format, key, path) in [
            (Format::Link, "md.link", "icons/md-link.svg"),
            (Format::Quote, "md.quote", "icons/md-quote.svg"),
            (
                Format::CodeBlock,
                "md.code_block",
                "icons/md-code-block.svg",
            ),
            (Format::Table, "md.table", "icons/md-table.svg"),
            (Format::Rule, "md.rule", "icons/md-rule.svg"),
            (Format::Image, "md.image", "icons/md-image.svg"),
        ] {
            bar = bar.child(
                Button::new(SharedString::from(key))
                    .small()
                    .ghost()
                    .icon(Icon::default().path(path))
                    .tooltip(t(cx, key))
                    .disabled(self.batch_locked)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.markdown_format(format, window, cx)
                    })),
            );
        }
        bar
    }
}
#[cfg(test)]
mod tests {
    use super::{Edit, Format, byte_offset, transform};
    fn apply(source: &str, edit: Edit) -> String {
        format!(
            "{}{}{}",
            &source[..edit.range.start],
            edit.text,
            &source[edit.range.end..]
        )
    }
    #[test]
    fn shortcuts_match_only_markdown_input_descendants() {
        let predicate = gpui::KeyBindingContextPredicate::parse("NoteMarkdown > Input").unwrap();
        let markdown = [
            gpui::KeyContext::parse("NoteMarkdown").unwrap(),
            gpui::KeyContext::parse("Input").unwrap(),
        ];
        assert!(predicate.eval_inner(&markdown, &markdown));
        for other in ["Workspace", "NoteCell", "Settings"] {
            let contexts = [
                gpui::KeyContext::parse(other).unwrap(),
                gpui::KeyContext::parse("Input").unwrap(),
            ];
            assert!(!predicate.eval_inner(&contexts, &contexts));
        }
    }
    #[test]
    fn unicode_wrapping_and_toggle_preserve_content() {
        let source = "甲😀乙";
        assert_eq!(byte_offset(source, 3), 7);
        assert_eq!(
            apply(source, transform(source, 3..7, Format::Bold)),
            "甲**😀**乙"
        );
        assert_eq!(
            apply("**中文**", transform("**中文**", 2..8, Format::Bold)),
            "中文"
        );
        assert_eq!(
            apply("**中文**", transform("**中文**", 2..8, Format::Italic)),
            "***中文***"
        );
    }
    #[test]
    fn multiline_lists_preserve_crlf_and_exclude_next_line() {
        let source = "第一行\r\n第二行\r\n第三行";
        let end = source.find("第三行").unwrap();
        let formatted = apply(source, transform(source, 0..end, Format::Numbers));
        assert_eq!(formatted, "1. 第一行\r\n2. 第二行\r\n第三行");
        assert_eq!(
            apply("- a\n- b", transform("- a\n- b", 0..7, Format::Bullets)),
            "a\nb"
        );
    }
    #[test]
    fn inline_code_retains_edge_backticks_and_insertions_keep_selection() {
        let source = "`literal`";
        let formatted = apply(source, transform(source, 0..source.len(), Format::Code));
        assert!(markdown::to_html(&formatted).contains("<code>`literal`</code>"));
        assert!(apply("原文", transform("原文", 0..6, Format::Rule)).contains("原文"));
        assert!(apply("原文", transform("原文", 0..6, Format::Table)).contains("原文"));
        assert_eq!(
            apply("中文", transform("中文", 0..6, Format::Heading(1))),
            "# 中文"
        );
    }
    #[test]
    fn headings_replace_and_fences_keep_literal_backticks() {
        assert_eq!(
            apply("## 标题", transform("## 标题", 0..0, Format::Heading(1))),
            "# 标题"
        );
        let edit = transform("a```b", 0..5, Format::CodeBlock);
        assert_eq!(edit.text, "````\na```b\n````");
        let edit = transform("中文", 0..6, Format::Link);
        assert_eq!(
            &edit.text[edit.caret..edit.caret + edit.select_ascii],
            "https://example.com"
        );
    }
}
