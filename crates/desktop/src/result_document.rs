//! Shared read-only result document. Formatting never changes the stored result values.
use crate::{assets, feedback, locale::t};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
};
use std::{collections::HashSet, io::Write, sync::Arc};
use turbodbn_services::database::workbench::Rows;
const LIMIT: usize = 256 * 1024;

pub fn icon(json: bool, cx: &App) -> Icon {
    Icon::default()
        .path(if json {
            "icons/result-json.svg"
        } else {
            "icons/result-grid.svg"
        })
        .text_color(assets::note_color(if json { 2 } else { 0 }, cx))
}
pub fn switches(
    json: bool,
    disabled: bool,
    cx: &App,
    change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> Div {
    let change = std::rc::Rc::new(change);
    div()
        .h_flex()
        .gap_1()
        .children([false, true].into_iter().map(|value| {
            let change = change.clone();
            Button::new(if value { "result-json" } else { "result-table" })
                .small()
                .ghost()
                .icon(icon(value, cx))
                .selected(json == value)
                .disabled(disabled)
                .tooltip(t(cx, if value { "result.json" } else { "result.table" }))
                .on_click(move |_, window, cx| change(value, window, cx))
        }))
}
pub fn single_json(rows: &Rows) -> bool {
    single(rows).is_some_and(|s| matches!(s.trim_start().chars().next(), Some('{' | '[')))
}
fn single(rows: &Rows) -> Option<&str> {
    (rows.columns.len() == 1 && rows.values.len() == 1)
        .then(|| rows.values[0].first().and_then(|v| v.as_deref()))
        .flatten()
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = bytes.len().min(LIMIT.saturating_sub(self.0.len()));
        self.0.extend_from_slice(&bytes[..n]);
        if n == bytes.len() {
            Ok(n)
        } else {
            Err(std::io::Error::other("preview limit"))
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn raw(rows: &Rows) -> String {
    single(rows)
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(rows).unwrap_or_default())
}
// Token-preserving indentation: do not round JSON numbers through a floating-point Value.
fn pretty(source: &str) -> (String, bool) {
    let mut de = serde_json::Deserializer::from_str(source);
    if source.len() > LIMIT
        || serde::de::IgnoredAny::deserialize(&mut de).is_err()
        || de.end().is_err()
    {
        let end = source
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= LIMIT)
            .last()
            .unwrap_or(0);
        return (
            if source.len() <= LIMIT {
                source.into()
            } else {
                source[..end].into()
            },
            source.len() > LIMIT,
        );
    }
    let mut out = String::new();
    let mut quoted = false;
    let mut escape = false;
    let mut depth = 0usize;
    let mut last = '\0';
    for ch in source.chars() {
        if quoted {
            out.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else {
            match ch {
                '"' => {
                    out.push(ch);
                    quoted = true;
                }
                '{' | '[' => {
                    out.push(ch);
                    depth += 1;
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
                '}' | ']' => {
                    depth = depth.saturating_sub(1);
                    if matches!(last, '{' | '[') {
                        while out.ends_with([' ', '\n']) {
                            out.pop();
                        }
                    } else {
                        out.push('\n');
                        out.push_str(&"  ".repeat(depth));
                    }
                    out.push(ch);
                }
                ',' => {
                    out.push_str(",\n");
                    out.push_str(&"  ".repeat(depth));
                }
                ':' => out.push_str(": "),
                ch if ch.is_whitespace() => continue,
                _ => out.push(ch),
            }
        }
        last = ch;
        if out.len() > LIMIT {
            let mut end = LIMIT;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            return (out, true);
        }
    }
    (out, false)
}
use serde::Deserialize;
struct Line {
    text: String,
    end: Option<usize>,
}
fn preview(rows: &Rows) -> (Vec<Line>, bool) {
    let (text, mut limited) = if let Some(value) = single(rows) {
        pretty(value)
    } else {
        let mut out = Bounded(Vec::new());
        let limited = serde_json::to_writer_pretty(&mut out, rows).is_err();
        (String::from_utf8_lossy(&out.0).into_owned(), limited)
    };
    let mut lines = Vec::<Line>::new();
    let mut stack = Vec::<usize>::new();
    for text in text.lines().take(10001) {
        if lines.len() == 10000 {
            limited = true;
            break;
        }
        let index = lines.len();
        if text.trim_start().starts_with(['}', ']'])
            && let Some(open) = stack.pop()
        {
            lines[open].end = Some(index);
        }
        if text.trim_end().ends_with(['{', '[']) {
            stack.push(index);
        }
        let mut chars = text.chars();
        let mut display = chars.by_ref().take(4096).collect::<String>();
        if chars.next().is_some() {
            display.push('…');
            limited = true;
        }
        lines.push(Line {
            text: display,
            end: None,
        });
    }
    (lines, limited)
}
pub struct ResultDocument {
    rows: Arc<Rows>,
    lines: Arc<Vec<Line>>,
    folded: HashSet<usize>,
    layout_revision: u64,
    visible: Option<(String, u64, Arc<Vec<usize>>)>,
    width: f32,
    measured: Option<(SharedString, Pixels, u64, String)>,
    search: Entity<InputState>,
    _search: Subscription,
    task: Option<Task<()>>,
    revision: u64,
    limited: bool,
    loading: bool,
    copying: bool,
    vertical: UniformListScrollHandle,
    horizontal: ScrollHandle,
}
impl ResultDocument {
    pub fn new(rows: Rows, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "result.search")));
        let mut this = Self {
            rows: Arc::new(Rows::default()),
            lines: Arc::default(),
            folded: HashSet::new(),
            layout_revision: 0,
            visible: None,
            width: 52.,
            measured: None,
            _search: cx.observe(&search, |_, _, cx| cx.notify()),
            search,
            task: None,
            revision: 0,
            limited: false,
            loading: false,
            copying: false,
            vertical: UniformListScrollHandle::new(),
            horizontal: ScrollHandle::new(),
        };
        this.set_rows(rows, cx);
        this
    }
    pub fn set_rows(&mut self, rows: Rows, cx: &mut Context<Self>) {
        self.rows = Arc::new(rows);
        self.revision += 1;
        self.loading = true;
        self.lines = Arc::default();
        self.folded.clear();
        self.layout_revision += 1;
        let rows = self.rows.clone();
        let revision = self.revision;
        let work = cx
            .background_executor()
            .spawn(async move { preview(&rows) });
        self.task = Some(cx.spawn(async move |view, cx| {
            let (lines, limited) = work.await;
            let _ = view.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.width = lines
                    .iter()
                    .map(|line| {
                        line.text
                            .chars()
                            .map(|c| if c.is_ascii() { 1 } else { 2 })
                            .sum::<usize>()
                    })
                    .max()
                    .unwrap_or(0) as f32
                    * 9.
                    + 52.;
                this.lines = Arc::new(lines);
                this.layout_revision += 1;
                this.limited = limited;
                this.loading = false;
                this.task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn copy(&mut self, cx: &mut Context<Self>) {
        if self.copying {
            return;
        }
        self.copying = true;
        let rows = self.rows.clone();
        let work = cx.background_executor().spawn(async move { raw(&rows) });
        cx.spawn(async move |view, cx| {
            let text = work.await;
            let _ = view.update(cx, |this, cx| {
                this.copying = false;
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                feedback::success(t(cx, "feedback.copied"), cx);
                cx.notify();
            });
        })
        .detach();
    }
}
impl ResultDocument {
    fn render_lines(&self, visible: Arc<Vec<usize>>, cx: &Context<Self>) -> AnyElement {
        use gpui_component::scroll::{Scrollbar, ScrollbarShow};
        let lines = self.lines.clone();
        let width = self.width;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                div()
                    .id("document-horizontal")
                    .size_full()
                    .track_scroll(&self.horizontal)
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .child(
                        uniform_list(
                            "document-lines",
                            visible.len(),
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|i| {
                                        let index = visible[i];
                                        let line = &lines[index];
                                        div()
                                            .h_flex()
                                            .h(px(22.))
                                            .w(px(width))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(cx.theme().mono_font_size)
                                            .child(
                                                Button::new(("fold-line", index))
                                                    .ghost()
                                                    .small()
                                                    .w(px(24.))
                                                    .h(px(22.))
                                                    .disabled(line.end.is_none())
                                                    .opacity(if line.end.is_some() {
                                                        1.
                                                    } else {
                                                        0.
                                                    })
                                                    .icon(if this.folded.contains(&index) {
                                                        IconName::ChevronRight
                                                    } else {
                                                        IconName::ChevronDown
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            if !this.folded.remove(&index) {
                                                                this.folded.insert(index);
                                                            }
                                                            this.layout_revision += 1;
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(div().whitespace_nowrap().child(
                                                if this.folded.contains(&index) {
                                                    format!(
                                                        "{} … {}",
                                                        line.text,
                                                        if line.text.ends_with('[') {
                                                            "]"
                                                        } else {
                                                            "}"
                                                        }
                                                    )
                                                } else {
                                                    line.text.clone()
                                                },
                                            ))
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(self.vertical.clone())
                        .h_full()
                        .w(px(width))
                        .min_w_full(),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(14.))
                    .child(
                        Scrollbar::vertical(&self.vertical).scrollbar_show(ScrollbarShow::Always),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(14.))
                    .child(
                        Scrollbar::horizontal(&self.horizontal)
                            .scrollbar_show(ScrollbarShow::Always),
                    ),
            )
            .into_any_element()
    }
}
impl Render for ResultDocument {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().to_lowercase();
        if self
            .visible
            .as_ref()
            .is_none_or(|(old, revision, _)| old != &query || *revision != self.layout_revision)
        {
            let mut hidden_until = None;
            let visible = self
                .lines
                .iter()
                .enumerate()
                .filter_map(|(i, line)| {
                    if !query.is_empty() {
                        return line.text.to_lowercase().contains(&query).then_some(i);
                    }
                    if hidden_until.is_some_and(|end| i <= end) {
                        return None;
                    }
                    if self.folded.contains(&i) {
                        hidden_until = line.end;
                    }
                    Some(i)
                })
                .collect::<Vec<_>>();
            self.visible = Some((query, self.layout_revision, Arc::new(visible)));
        }
        let visible = self.visible.as_ref().unwrap().2.clone();
        let signature = (
            cx.theme().mono_font_family.clone(),
            cx.theme().mono_font_size,
            self.layout_revision,
            self.search.read(cx).value().to_string(),
        );
        if self.measured.as_ref() != Some(&signature) {
            let font = gpui::font(signature.0.clone());
            self.width = visible
                .iter()
                .map(|i| {
                    let line = &self.lines[*i];
                    let text: SharedString = if self.folded.contains(i) {
                        format!("{} … }}", line.text).into()
                    } else {
                        line.text.clone().into()
                    };
                    let run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: cx.theme().foreground,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    f32::from(
                        window
                            .text_system()
                            .shape_line(text, signature.1, &[run], None)
                            .width,
                    )
                })
                .fold(0., f32::max)
                + 38.;
            self.measured = Some(signature);
        }

        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(Input::new(&self.search).small().cleanable(true).flex_1())
                    .child(
                        Button::new("document-fold")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronsUpDown)
                            .tooltip(t(cx, "result.fold"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.folded.is_empty() {
                                    this.folded.extend(
                                        this.lines
                                            .iter()
                                            .enumerate()
                                            .filter_map(|(i, l)| l.end.map(|_| i)),
                                    );
                                } else {
                                    this.folded.clear();
                                }
                                this.layout_revision += 1;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("document-copy")
                            .small()
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip(t(cx, "result.copy"))
                            .disabled(self.copying)
                            .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.loading {
                        t(cx, "dbwork.loading")
                    } else if self.limited {
                        t(cx, "result.limited")
                    } else if single(&self.rows).is_some() {
                        t(cx, "result.single")
                    } else {
                        crate::locale::tr(
                            cx,
                            "result.page",
                            &[("rows", &self.rows.values.len().to_string())],
                        )
                    }),
            )
            .child(self.render_lines(visible, cx))
    }
}
#[cfg(test)]
mod tests {
    use super::{LIMIT, pretty, preview, raw, single_json};
    use turbodbn_services::database::workbench::Rows;
    #[test]
    fn preserves_numbers_duplicate_names_nulls_and_full_copy() {
        let json = r#"{"id":123456789012345678901,"value":0.1234567890123456789,"text":"a\\b"}"#;
        let rows = Rows {
            columns: vec!["plan".into()],
            values: vec![vec![Some(json.into())]],
        };
        assert_eq!(raw(&rows), json);
        assert!(single_json(&rows));
        let (pretty, limited) = pretty(json);
        assert!(
            !limited
                && pretty.contains("123456789012345678901")
                && pretty.contains("0.1234567890123456789")
        );
        let rows = Rows {
            columns: vec!["id".into(), "id".into()],
            values: vec![vec![None, Some("1".into())]],
        };
        let value: serde_json::Value = serde_json::from_str(&raw(&rows)).unwrap();
        assert_eq!(value["columns"], serde_json::json!(["id", "id"]));
        assert_eq!(value["values"][0][0], serde_json::Value::Null);
        let rows = Rows {
            columns: vec!["value".into()],
            values: vec![vec![Some("x".repeat(LIMIT * 2))]],
        };
        assert!(preview(&rows).1);
        assert_eq!(raw(&rows).len(), LIMIT * 2);
    }
}
