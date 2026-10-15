//! Virtual, lazy EJSON nodes. Editing only changes local memory until explicit confirmation.
use crate::locale::t;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::InputState,
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use turbodbn_core::mongo::{self, Mutation, Node, Projection};
#[derive(Clone)]
pub enum Event {
    Changed,
    Submit(Mutation),
}
pub struct MongoTree {
    data: Arc<Projection>,
    row: usize,
    collection: String,
    can_write: bool,
    working: Option<Value>,
    clone_document: bool,
    open: HashSet<String>,
    limits: HashMap<String, usize>,
    nodes: Arc<Vec<Node>>,
    limited: bool,
    scroll: UniformListScrollHandle,
    selected: String,
    busy: bool,
    revision: u64,
}
impl EventEmitter<Event> for MongoTree {}
impl MongoTree {
    pub fn new(data: Arc<Projection>, collection: String, can_write: bool) -> Self {
        let mut this = Self {
            data,
            row: 0,
            collection,
            can_write,
            working: None,
            clone_document: false,
            open: HashSet::from([String::new()]),
            limits: HashMap::new(),
            nodes: Default::default(),
            limited: false,
            scroll: UniformListScrollHandle::new(),
            selected: String::new(),
            busy: false,
            revision: 0,
        };
        this.rebuild();
        this
    }
    pub fn matches(&self, change: &Mutation) -> bool {
        self.working.as_ref().is_some_and(|v| {
            serde_json::from_str::<Value>(&change.replacement).is_ok_and(|new| *v == new)
        }) && self.data.documents.get(self.row).is_some_and(|v| {
            serde_json::from_str::<Value>(&change.original).is_ok_and(|old| *v == old)
        }) && self.clone_document == change.clone_document
    }
    pub fn dirty(&self) -> bool {
        self.working.is_some()
    }
    pub fn busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }
    pub fn finish(&mut self, success: bool, cx: &mut Context<Self>) {
        self.busy = false;
        if success {
            self.working = None;
            self.clone_document = false;
            self.revision += 1;
            cx.emit(Event::Changed);
        }
        cx.notify();
    }
    pub fn select(&mut self, row: usize, path: String, cx: &mut Context<Self>) -> bool {
        if row >= self.data.documents.len() {
            return false;
        }
        if self.row != row && self.dirty() {
            crate::feedback::error(t(cx, "mongo.document.pending"), cx);
            return false;
        }
        if self.row != row {
            self.row = row;
            self.open = HashSet::from([String::new()]);
            self.limits.clear();
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.selected = path.clone();
        let mut parent = path.as_str();
        while let Some((head, _)) = parent.rsplit_once('/') {
            self.open.insert(head.to_owned());
            parent = head;
        }
        self.rebuild();
        if let Some(index) = self.nodes.iter().position(|n| n.path == path) {
            self.scroll.scroll_to_item(index, ScrollStrategy::Top);
        }
        cx.notify();
        true
    }
    fn document(&self) -> Option<&Value> {
        self.working
            .as_ref()
            .or_else(|| self.data.documents.get(self.row))
    }
    fn rebuild(&mut self) {
        let (nodes, limited) = self
            .document()
            .map(|d| mongo::nodes(d, &self.open, &self.limits))
            .unwrap_or_default();
        self.nodes = Arc::new(nodes);
        self.limited = limited;
    }
    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.open.remove(path) {
            self.open.insert(path.into());
        }
        self.rebuild();
        cx.notify();
    }
    fn stage(&mut self, path: &str, value: Option<Value>, cx: &mut Context<Self>) {
        if !self.can_write || self.busy {
            return;
        }
        let Some(doc) = self.document() else {
            return;
        };
        match mongo::apply(doc, path, value) {
            Ok(value) => {
                self.working = Some(value);
                self.revision += 1;
                self.rebuild();
                cx.emit(Event::Changed);
                cx.notify();
            }
            Err(key) => crate::feedback::error(t(cx, key), cx),
        }
    }
    fn copy(&self, path: &str, action: &str, cx: &mut App) {
        let Some(doc) = self.document() else {
            return;
        };
        let Some(value) = doc.pointer(path) else {
            return;
        };
        let text = match action {
            "name" if path.is_empty() => "document".into(),
            "name" => path
                .rsplit('/')
                .next()
                .unwrap_or("document")
                .replace("~1", "/")
                .replace("~0", "~"),
            "path" => path.into(),
            "shell" => mongo::shell(doc),
            "document" => serde_json::to_string_pretty(doc).unwrap_or_default(),
            _ => serde_json::to_string_pretty(value).unwrap_or_default(),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        crate::feedback::success(t(cx, "feedback.copied"), cx);
    }
    fn edit(
        &mut self,
        path: String,
        clone_document: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_write || self.busy || (clone_document && self.dirty()) {
            return;
        }
        let Some(mut value) = self.document().and_then(|d| d.pointer(&path)).cloned() else {
            return;
        };
        if clone_document && let Some(map) = value.as_object_mut() {
            map.remove("_id");
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .code_editor("json")
                .default_value(serde_json::to_string_pretty(&value).unwrap_or_default())
        });
        let owner = cx.entity().downgrade();
        let revision = self.revision;
        let row = self.row;
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let owner = owner.clone();
            let path = path.clone();
            dialog
                .title(t(
                    cx,
                    if clone_document {
                        "mongo.document.clone"
                    } else {
                        "mongo.node.edit"
                    },
                ))
                .w(px(740.))
                .overlay_closable(false)
                .child(
                    div()
                        .w(px(690.))
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "mongo.node.edit_hint"))
                        .child(crate::feedback::editable_multiline(&input, false, cx).h(px(280.))),
                )
                .footer(move |_, _, _, cx| {
                    let input = input.clone();
                    let owner = owner.clone();
                    let path = path.clone();
                    vec![
                        crate::feedback::footer_actions()
                            .child(crate::feedback::close_button(cx))
                            .child(
                                Button::new("mongo-stage")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "mongo.node.stage"))
                                    .on_click(move |_, window, cx| {
                                        let source = input.read(cx).value();
                                        if source.len() > mongo::DOCUMENT_BYTES {
                                            crate::feedback::error(
                                                t(cx, "mongo.document.limit"),
                                                cx,
                                            );
                                            return;
                                        }
                                        let Ok(value) = serde_json::from_str::<Value>(&source)
                                        else {
                                            crate::feedback::error(
                                                t(cx, "mongo.document.invalid"),
                                                cx,
                                            );
                                            return;
                                        };
                                        let applied = owner.update(cx, |this, cx| {
                                            if this.revision != revision
                                                || this.row != row
                                                || this.busy
                                            {
                                                return false;
                                            }
                                            if clone_document {
                                                if !value.is_object() || value.get("_id").is_some()
                                                {
                                                    crate::feedback::error(
                                                        t(cx, "mongo.document.id_immutable"),
                                                        cx,
                                                    );
                                                    return false;
                                                }
                                                this.working = Some(value.clone());
                                                this.clone_document = true;
                                                this.revision += 1;
                                                this.rebuild();
                                                cx.emit(Event::Changed);
                                                cx.notify();
                                            } else {
                                                if let Some(document) = this.document()
                                                    && let Err(key) = mongo::apply(
                                                        document,
                                                        &path,
                                                        Some(value.clone()),
                                                    )
                                                {
                                                    crate::feedback::error(t(cx, key), cx);
                                                    return false;
                                                }
                                                this.stage(&path, Some(value.clone()), cx);
                                            }
                                            true
                                        });
                                        if applied.unwrap_or(false) {
                                            window.close_dialog(cx);
                                        }
                                    }),
                            ),
                    ]
                })
        });
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.can_write {
            return;
        }
        if let (Some(old), Some(new)) = (self.data.documents.get(self.row), self.working.as_ref()) {
            let change = Mutation {
                collection: self.collection.clone(),
                original: old.to_string(),
                replacement: new.to_string(),
                clone_document: self.clone_document,
                confirmed: true,
            };
            if let Err(key) = mongo::validate_mutation(&change) {
                crate::feedback::error(t(cx, key), cx);
                return;
            }
            cx.emit(Event::Submit(change));
        }
    }
    fn menu_for(
        owner: WeakEntity<Self>,
        path: String,
        mut menu: gpui_component::menu::PopupMenu,
        cx: &mut Context<gpui_component::menu::PopupMenu>,
    ) -> gpui_component::menu::PopupMenu {
        let Some(view) = owner.upgrade() else {
            return menu;
        };
        let this = view.read(cx);
        let expandable = this
            .document()
            .and_then(|d| d.pointer(&path))
            .is_some_and(mongo::expandable);
        let editable = this.can_write && !this.busy && path != "/_id" && !path.starts_with("/_id/");
        let clone_allowed = this.can_write && !this.busy && !this.dirty();
        for (key, action) in [
            ("mongo.copy.value", "value"),
            ("mongo.copy.name", "name"),
            ("mongo.copy.path", "path"),
            ("mongo.copy.document", "document"),
            ("mongo.copy.shell", "shell"),
        ] {
            let owner = owner.clone();
            let path = path.clone();
            menu = menu.item(
                gpui_component::menu::PopupMenuItem::new(t(cx, key))
                    .icon(IconName::Copy)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| this.copy(&path, action, cx));
                    }),
            );
        }
        for (key, action) in [
            ("mongo.node.expand", "expand"),
            ("mongo.node.collapse", "collapse"),
            ("mongo.node.expand_children", "children"),
            ("mongo.node.more", "more"),
            ("mongo.node.collapse_all", "all"),
        ] {
            let owner = owner.clone();
            let path = path.clone();
            menu = menu.item(
                gpui_component::menu::PopupMenuItem::new(t(cx, key))
                    .disabled(!expandable && action != "all")
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            match action {
                                "expand" => {
                                    this.open.insert(path.clone());
                                }
                                "collapse" => {
                                    this.open.remove(&path);
                                }
                                "all" => this.open.clear(),
                                "more" => {
                                    this.open.insert(path.clone());
                                    let n = this.limits.entry(path.clone()).or_insert(100);
                                    *n = (*n + 100).min(mongo::MAX_NODES);
                                }
                                _ => {
                                    this.open.insert(path.clone());
                                    let children = this
                                        .document()
                                        .and_then(|d| d.pointer(&path))
                                        .map(|v| match v {
                                            Value::Object(map) => map
                                                .keys()
                                                .take(100)
                                                .map(|k| mongo::child_path(&path, k))
                                                .collect::<Vec<_>>(),
                                            Value::Array(a) => (0..a.len().min(100))
                                                .map(|i| mongo::child_path(&path, &i.to_string()))
                                                .collect(),
                                            _ => vec![],
                                        })
                                        .unwrap_or_default();
                                    this.open.extend(children);
                                }
                            }
                            this.rebuild();
                            cx.notify();
                        });
                    }),
            );
        }
        for (key, action) in [
            ("mongo.node.edit", "edit"),
            ("mongo.node.null", "null"),
            ("mongo.node.delete", "delete"),
            ("mongo.document.clone", "clone"),
        ] {
            let owner = owner.clone();
            let path = path.clone();
            menu = menu.item(
                gpui_component::menu::PopupMenuItem::new(t(cx, key))
                    .disabled(if action == "clone" {
                        !clone_allowed || !path.is_empty()
                    } else {
                        !editable || (action != "edit" && path.is_empty())
                    })
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| match action {
                            "edit" => this.edit(path.clone(), false, window, cx),
                            "clone" => this.edit(String::new(), true, window, cx),
                            "null" => this.stage(&path, Some(Value::Null), cx),
                            _ => this.stage(&path, None, cx),
                        });
                    }),
            );
        }
        menu
    }
}
impl Render for MongoTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nodes = self.nodes.clone();
        let owner = cx.entity().downgrade();
        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .gap_1()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("mongo-prev-document")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronLeft)
                            .disabled(self.row == 0 || self.dirty() || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select(this.row - 1, String::new(), cx);
                            })),
                    )
                    .child(crate::locale::tr(
                        cx,
                        "mongo.document.position",
                        &[
                            ("index", &(self.row + 1).to_string()),
                            ("count", &self.data.documents.len().to_string()),
                        ],
                    ))
                    .child(
                        Button::new("mongo-next-document")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronRight)
                            .disabled(
                                self.row + 1 >= self.data.documents.len()
                                    || self.dirty()
                                    || self.busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select(this.row + 1, String::new(), cx);
                            })),
                    )
                    .child(div().flex_1())
                    .when(self.dirty(), |el| {
                        el.child(t(cx, "mongo.document.pending_short"))
                    })
                    .child(
                        Button::new("mongo-undo")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label(t(cx, "mongo.node.undo"))
                            .disabled(!self.dirty() || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.working = None;
                                this.clone_document = false;
                                this.revision += 1;
                                this.rebuild();
                                cx.emit(Event::Changed);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("mongo-submit")
                            .small()
                            .primary()
                            .icon(IconName::Check)
                            .label(t(cx, "mongo.document.preview"))
                            .disabled(!self.dirty() || !self.can_write || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .when(!self.can_write, |el| {
                el.child(div().text_xs().child(t(cx, "mongo.document.readonly")))
            })
            .when(self.limited, |el| {
                el.child(div().text_xs().child(t(cx, "mongo.node.limited")))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "mongo-tree",
                            nodes.len(),
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|i| {
                                        let node = &nodes[i];
                                        let path = node.path.clone();
                                        let clicked = path.clone();
                                        let toggle = path.clone();
                                        let menu_path = path.clone();
                                        let menu_owner = owner.clone();
                                        let row = div()
                                            .id(("mongo-node-row", i))
                                            .h_flex()
                                            .items_center()
                                            .h(px(26.))
                                            .pl(px(node.depth as f32 * 16.))
                                            .gap_2()
                                            .overflow_hidden()
                                            .when(this.selected == path, |el| {
                                                el.bg(cx.theme().accent)
                                            })
                                            .child(
                                                Button::new(("mongo-node-toggle", i))
                                                    .ghost()
                                                    .xsmall()
                                                    .icon(if this.open.contains(&path) {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .disabled(!node.expandable)
                                                    .opacity(if node.expandable { 1. } else { 0. })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.toggle(&toggle, cx)
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .max_w(px(240.))
                                                    .text_ellipsis()
                                                    .overflow_hidden()
                                                    .child(node.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(node.kind),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .overflow_hidden()
                                                    .child(node.summary.clone()),
                                            )
                                            .on_click(cx.listener(
                                                move |this, event: &ClickEvent, window, cx| {
                                                    this.selected = clicked.clone();
                                                    if event.click_count() == 2 {
                                                        this.edit(
                                                            clicked.clone(),
                                                            false,
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                    cx.notify();
                                                },
                                            ));
                                        crate::context_menu::cell(
                                            SharedString::from(format!("mongo-node-menu-{i}")),
                                            row,
                                            move |menu, _, cx| {
                                                Self::menu_for(
                                                    menu_owner.clone(),
                                                    menu_path.clone(),
                                                    menu,
                                                    cx,
                                                )
                                            },
                                        )
                                        .into_any_element()
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(self.scroll.clone())
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(14.))
                            .child(
                                gpui_component::scroll::Scrollbar::vertical(&self.scroll)
                                    .scrollbar_show(gpui_component::scroll::ScrollbarShow::Always),
                            ),
                    ),
            )
    }
}
