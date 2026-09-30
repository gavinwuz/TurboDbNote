use super::*;
use crate::locale::t;
use markdown::mdast::Node;

#[derive(Clone)]
pub(super) enum Block {
    Markdown(String),
    Table(Vec<Vec<String>>),
    Image(String, String),
    List(Vec<(String, Vec<Block>)>),
}

fn source_text(node: &Node, source: &str) -> String {
    node.position()
        .and_then(|p| source.get(p.start.offset..p.end.offset))
        .unwrap_or_default()
        .into()
}

// Display-only normalization. The stored response and copy-all source remain untouched.
fn paragraph_text(node: &Node, source: &str) -> String {
    let mut text = source_text(node, source);
    let Some(position) = node.position() else {
        return text;
    };
    let mut replacements = vec![];
    fn collect(node: &Node, origin: usize, edits: &mut Vec<(std::ops::Range<usize>, String)>) {
        if let Node::Html(html) = node {
            if ["<br>", "<br/>", "<br />"]
                .contains(&html.value.trim().to_ascii_lowercase().as_str())
            {
                return;
            }
            if let Some(pos) = &html.position {
                edits.push((
                    pos.start.offset - origin..pos.end.offset - origin,
                    html.value
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;"),
                ));
            }
        } else if let Some(children) = node.children() {
            for child in children {
                collect(child, origin, edits);
            }
        }
    }
    collect(node, position.start.offset, &mut replacements);
    for (range, replacement) in replacements.into_iter().rev() {
        text.replace_range(range, &replacement);
    }
    text
}

fn list_items(node: &markdown::mdast::List, source: &str) -> Vec<(String, Vec<Block>)> {
    node.children
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let Node::ListItem(item) = item else {
                return None;
            };
            let marker = if let Some(checked) = item.checked {
                if checked { "☑".into() } else { "☐".into() }
            } else if node.ordered {
                format!("{}.", node.start.unwrap_or(1) as usize + index)
            } else {
                "•".into()
            };
            let mut parts = vec![];
            for (part_index, child) in item.children.iter().enumerate() {
                match child {
                    Node::List(list) => parts.push(Block::List(list_items(list, source))),
                    Node::Paragraph(p) => {
                        // Leading bold is a section title only when ordinary prose follows it.
                        if part_index == 0
                            && p.children.len() > 1
                            && matches!(p.children.first(), Some(Node::Strong(_)))
                        {
                            let first = &p.children[0];
                            parts.push(Block::Markdown(paragraph_text(first, source)));
                            let mut body = p.clone();
                            body.children.remove(0);
                            if let Some(pos) = &mut body.position {
                                pos.start = body.children[0].position().unwrap().start.clone();
                            }
                            parts.push(Block::Markdown(paragraph_text(
                                &Node::Paragraph(body),
                                source,
                            )));
                        } else {
                            parts.push(Block::Markdown(paragraph_text(child, source)));
                        }
                    }
                    _ => parts.extend(blocks(&source_text(child, source))),
                }
            }
            Some((marker, parts))
        })
        .collect()
}

fn blocks(source: &str) -> Vec<Block> {
    let Ok(Node::Root(root)) = markdown::to_mdast(source, &markdown::ParseOptions::gfm()) else {
        return vec![Block::Markdown(source.into())];
    };
    let mut blocks = vec![];
    let mut start = 0;
    for node in &root.children {
        let Some(pos) = node.position() else {
            continue;
        };
        let special = match node {
            Node::List(list) => Some(Block::List(list_items(list, source))),
            Node::Paragraph(p)
                if !(p.children.len() == 1 && matches!(&p.children[0], Node::Image(_))) =>
            {
                Some(Block::Markdown(paragraph_text(node, source)))
            }
            Node::Table(table) if table.children.len() <= 201 => {
                let rows = table
                    .children
                    .iter()
                    .filter_map(|row| {
                        if let Node::TableRow(row) = row {
                            Some(row)
                        } else {
                            None
                        }
                    })
                    .map(|row| {
                        row.children
                            .iter()
                            .take(64)
                            .map(|cell| {
                                if let Node::TableCell(cell) = cell {
                                    cell.children
                                        .first()
                                        .and_then(Node::position)
                                        .zip(cell.children.last().and_then(Node::position))
                                        .and_then(|(first, last)| {
                                            source.get(first.start.offset..last.end.offset)
                                        })
                                        .unwrap_or_default()
                                        .to_string()
                                } else {
                                    String::new()
                                }
                            })
                            .collect()
                    })
                    .collect();
                Some(Block::Table(rows))
            }
            Node::Paragraph(p) if p.children.len() == 1 => match &p.children[0] {
                Node::Image(image) => Some(Block::Image(image.alt.clone(), image.url.clone())),
                _ => None,
            },
            _ => None,
        };
        if let Some(block) = special {
            if start < pos.start.offset && !source[start..pos.start.offset].trim().is_empty() {
                blocks.push(Block::Markdown(source[start..pos.start.offset].into()));
            }
            blocks.push(block);
            start = pos.end.offset;
        }
    }
    if start < source.len() && !source[start..].trim().is_empty() {
        blocks.push(Block::Markdown(source[start..].into()));
    }
    blocks
}

impl ChatView {
    pub(super) fn rich_message(
        &self,
        index: usize,
        source: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let blocks = {
            let mut cache = self.markup_cache.borrow_mut();
            if let Some((_, blocks)) = cache.iter().find(|(text, _)| text == &source) {
                blocks.clone()
            } else {
                let parsed = if source.len() <= 131072 {
                    blocks(&source)
                } else {
                    vec![Block::Markdown(source.to_string())]
                };
                while cache.len() >= 32
                    || cache.iter().map(|(text, _)| text.len()).sum::<usize>() + source.len()
                        > 1048576
                {
                    if cache.pop_front().is_none() {
                        break;
                    }
                }
                if source.len() <= 131072 {
                    cache.push_back((source, parsed.clone()));
                }
                parsed
            }
        };
        self.render_message_blocks(&format!("chat-{index}"), blocks, window, cx)
    }
    fn render_message_blocks(
        &self,
        prefix: &str,
        blocks: Vec<Block>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .v_flex()
            .gap_3()
            .w_full()
            .min_w_0()
            .line_height(relative(1.65));
        for (block_index, block) in blocks.into_iter().enumerate() {
            let id = SharedString::from(format!("{prefix}-block-{block_index}"));
            body = match block {
                Block::List(items) => {
                    let mut list = div().v_flex().gap_4().w_full().min_w_0();
                    for (item_index, (marker, parts)) in items.into_iter().enumerate() {
                        list = list.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_start()
                                .gap_2()
                                .w_full()
                                .child(
                                    div()
                                        .min_w(px(22.))
                                        .flex_shrink_0()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(marker),
                                )
                                .child(div().flex_1().min_w_0().child(self.render_message_blocks(
                                    &format!("{id}-item-{item_index}"),
                                    parts,
                                    window,
                                    cx,
                                ))),
                        );
                    }
                    body.child(list)
                }
                Block::Markdown(text) => body.child(
                    TextView::markdown(id, text, window, cx)
                        .w_full()
                        .min_w_0()
                        .selectable(true)
                        .style(
                            gpui_component::text::TextViewStyle::default()
                                .paragraph_gap(rems(0.6))
                                .code_block(StyleRefinement::default().w_full().pr(px(40.))),
                        )
                        .code_block_actions(move |block, _, cx| {
                            let text = block.code().to_string();
                            Button::new("copy-code")
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip(t(cx, "chat.action.copy_code"))
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                })
                        }),
                ),
                Block::Table(rows) => {
                    let count = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let widths = (0..count)
                        .map(|column| {
                            (rows
                                .iter()
                                .filter_map(|r| r.get(column))
                                .map(|s| s.chars().count())
                                .max()
                                .unwrap_or(8) as f32
                                * 8.
                                + 24.)
                                .clamp(100., 320.)
                        })
                        .collect::<Vec<_>>();
                    let mut table = div()
                        .v_flex()
                        .w(px(widths.iter().sum()))
                        .border_1()
                        .border_color(cx.theme().border);
                    for (r, row) in rows.iter().enumerate() {
                        let mut line = div()
                            .flex()
                            .flex_row()
                            .when(r == 0, |line| line.bg(cx.theme().muted));
                        for (c, width) in widths.iter().enumerate() {
                            line = line.child(
                                div()
                                    .w(px(*width))
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .px_2()
                                    .py_1()
                                    .border_r_1()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        TextView::markdown(
                                            SharedString::from(format!("{id}-{r}-{c}")),
                                            row.get(c).cloned().unwrap_or_default(),
                                            window,
                                            cx,
                                        )
                                        .selectable(true),
                                    ),
                            );
                        }
                        table = table.child(line);
                    }
                    let copy = rows
                        .iter()
                        .map(|row| row.join("\t"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    body.child(
                        div().h_flex().justify_end().child(
                            Button::new(("copy-chat-table", block_index))
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip(t(cx, "chat.copy_table"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                }),
                        ),
                    )
                    .child(
                        div()
                            .id(id)
                            .w_full()
                            .min_w_0()
                            .overflow_x_scroll()
                            .child(table),
                    )
                }
                Block::Image(alt, url) => {
                    let safe = reqwest::Url::parse(&url).ok().filter(|u| {
                        matches!(u.scheme(), "https" | "http")
                            && u.username().is_empty()
                            && u.password().is_none()
                    });
                    body.child(
                        div()
                            .v_flex()
                            .gap_1()
                            .p_2()
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(if alt.is_empty() {
                                t(cx, "chat.image").to_string()
                            } else {
                                alt
                            })
                            .child(
                                Button::new(("chat-image", block_index))
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .label(t(cx, "chat.open_image"))
                                    .disabled(safe.is_none())
                                    .on_click(move |_, _, cx| {
                                        if let Some(url) = &safe {
                                            cx.open_url(url.as_str());
                                        }
                                    }),
                            ),
                    )
                }
            };
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Block, blocks};
    #[test]
    fn separates_list_titles_and_preserves_inline_code_and_unknown_html() {
        let source = "1. **高频查询可评估覆盖索引**若使用 InnoDB，且 `user_id` 是主键，可考虑 `(area_id, user_name)`。\n2. **检查字段类型**保留 <unknown> 与中文正文。";
        let parsed = blocks(source);
        let Block::List(items) = &parsed[0] else {
            panic!("Expected list");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "1.");
        assert!(matches!(&items[0].1[0], Block::Markdown(s) if s == "**高频查询可评估覆盖索引**"));
        assert!(
            matches!(&items[0].1[1], Block::Markdown(s) if s.contains("`(area_id, user_name)`"))
        );
        assert!(matches!(&items[1].1[1], Block::Markdown(s) if s.contains("&lt;unknown&gt;")));
        assert!(source.contains("<unknown>"));
    }
    #[test]
    fn parses_tables_without_interpreting_code_as_table() {
        let parsed = blocks(
            "intro\n\n| a | b |\n|---|---|\n| x | y |\n\n```sql\nSELECT 'a|b';\n```\n\n![diagram](https://example.invalid/a.png)",
        );
        let table = parsed
            .iter()
            .find_map(|block| {
                if let Block::Table(rows) = block {
                    Some(rows)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(table[0], vec!["a", "b"]);
        assert_eq!(table[1], vec!["x", "y"]);
        assert_eq!(
            parsed
                .iter()
                .filter(|b| matches!(b, Block::Table(_)))
                .count(),
            1
        );
        assert!(
            parsed
                .iter()
                .any(|b| matches!(b, Block::Markdown(s) if s.contains("SELECT 'a|b'")))
        );
        assert!(parsed.iter().any(|b| matches!(b, Block::Image(_, _))));
    }
}
