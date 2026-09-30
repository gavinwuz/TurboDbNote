use super::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Navigation {
    pub order: Vec<String>,
    pub environment_order: Vec<String>,
    pub mode: GroupMode,
    pub recent: Vec<Uuid>,
    pub databases: HashMap<Uuid, String>,
    pub created: HashMap<Uuid, String>,
    pub categories: Vec<String>,
    pub environments: Vec<String>,
    pub top_height: f32,
    pub top_ratio: f32,
    pub collapsed: std::collections::HashSet<String>,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum GroupMode {
    #[default]
    Group,
    Environment,
    Date,
    UsedDate,
}
impl GroupMode {
    fn add_key(self) -> &'static str {
        if self == Self::Group {
            "dbwork.project.add"
        } else {
            "dbwork.environment.add"
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Group => "dbwork.group",
            Self::Environment => "dbwork.environment",
            Self::Date => "dbwork.date",
            Self::UsedDate => "feedback.group.used",
        }
    }
}
pub(super) fn load(path: &Path) -> Result<Navigation, String> {
    crate::app_store::load(path, "navigation", 65536, |_: &Navigation| true)
}

pub(super) fn writer(
    path: PathBuf,
) -> (
    async_channel::Sender<Navigation>,
    async_channel::Receiver<String>,
) {
    let (send, receive) = async_channel::bounded::<Navigation>(1);
    let (errors, out) = async_channel::unbounded();
    std::thread::spawn(move || {
        while let Ok(state) = receive.recv_blocking() {
            let result = crate::app_store::save(&path, "navigation", &state, 65536);
            if let Err(error) = result {
                let _ = errors.try_send(format!("导航排序保存失败：{error}"));
            }
        }
    });
    (send, out)
}
#[derive(Clone)]
struct GroupDrag(String);
impl Render for GroupDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_2().child(self.0.clone())
    }
}
// Keep historical grouping keys so existing order/collapse preferences survive the terminology change.
fn group(p: &Profile) -> String {
    if p.group.trim().is_empty() {
        "未分组".into()
    } else {
        p.group.clone()
    }
}
pub(super) fn environment_color(name: &str) -> Hsla {
    let n = name.to_lowercase();
    if n.contains("正式") || n.contains("生产") || n == "prod" {
        hsla(0., 0.7, 0.55, 1.)
    } else if n.contains("预发") || n == "staging" {
        hsla(0.09, 0.8, 0.5, 1.)
    } else if n.contains("测试") || n == "test" {
        hsla(0.6, 0.7, 0.6, 1.)
    } else if n.contains("开发") || n == "dev" {
        hsla(0.4, 0.6, 0.45, 1.)
    } else {
        hsla(0., 0., 0.55, 1.)
    }
}
pub(super) fn type_icon(kind: DatabaseKind) -> gpui_component::Icon {
    gpui_component::Icon::default()
        .path(match kind {
            DatabaseKind::MySql => "icons/db-mysql.svg",
            DatabaseKind::PostgreSql => "icons/db-postgres.svg",
            DatabaseKind::Sqlite => "icons/db-sqlite.svg",
            DatabaseKind::ClickHouse => "icons/db-clickhouse.svg",
            DatabaseKind::Redis => "icons/db-redis.svg",
        })
        .text_color(match kind {
            DatabaseKind::MySql => hsla(0.56, 0.7, 0.5, 1.),
            DatabaseKind::Sqlite => hsla(0.62, 0.5, 0.6, 1.),
            DatabaseKind::PostgreSql => hsla(0.63, 0.7, 0.5, 1.),
            DatabaseKind::ClickHouse => hsla(0.14, 0.8, 0.45, 1.),
            DatabaseKind::Redis => hsla(0., 0.8, 0.6, 1.),
        })
}
impl DatabaseView {
    pub(super) fn persist_navigation(&self) {
        if self.navigation_error {
            return;
        }
        let _ = self.nav_writer.force_send(self.navigation.clone());
    }
    fn group_key(&self, p: &Profile) -> String {
        match self.navigation.mode {
            GroupMode::Group => group(p),
            GroupMode::Environment => {
                if p.environment.is_empty() {
                    "未设置环境".into()
                } else {
                    p.environment.clone()
                }
            }
            GroupMode::Date => connection_store::day(
                self.history
                    .get(&p.id)
                    .and_then(|h| h.created_at.as_deref()),
            )
            .unwrap_or_else(|| "日期未知".into()),
            GroupMode::UsedDate => connection_store::day(
                self.history
                    .get(&p.id)
                    .and_then(|h| h.last_used_at.as_deref()),
            )
            .unwrap_or_else(|| "日期未知".into()),
        }
    }
    fn ordered_groups(&self) -> Vec<String> {
        let mut names = self
            .profiles
            .iter()
            .map(|p| self.group_key(p))
            .collect::<Vec<_>>();
        match self.navigation.mode {
            GroupMode::Group => names.extend(self.navigation.categories.clone()),
            GroupMode::Environment => names.extend(self.navigation.environments.clone()),
            _ => {}
        }
        names.sort();
        names.dedup();
        if matches!(self.navigation.mode, GroupMode::Date | GroupMode::UsedDate) {
            names.reverse();
            if let Some(index) = names.iter().position(|n| n == "日期未知") {
                let unknown = names.remove(index);
                names.push(unknown);
            }
            return names;
        }
        let order = if self.navigation.mode == GroupMode::Environment {
            &self.navigation.environment_order
        } else {
            &self.navigation.order
        };
        let mut ordered = order
            .iter()
            .filter(|n| names.contains(n))
            .cloned()
            .collect::<Vec<_>>();
        for name in names {
            if !ordered.contains(&name) {
                ordered.push(name);
            }
        }
        ordered
    }
    pub(super) fn remember_connection(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.profiles.iter().any(|p| p.id == id) {
            let at = connection_store::timestamp();
            let history = self.history.entry(id).or_default();
            if history.last_used_at.as_ref().is_none_or(|old| old < &at) {
                history.last_used_at = Some(at.clone());
            }
            let path = self.file.clone();
            let task = cx
                .background_executor()
                .spawn(async move { connection_store::mark_used(&path, id, &at) });
            cx.spawn(async move |view, cx| {
                if let Err(error) = task.await {
                    let _ = view.update(cx, |_, cx| feedback::error(error, cx));
                }
            })
            .detach();
        }
        self.navigation.recent.retain(|old| *old != id);
        self.navigation.recent.insert(0, id);
        self.navigation.recent.truncate(5);
        self.persist_navigation();
    }
    pub fn navigation_menu(
        &self,
        menu: gpui_component::menu::PopupMenu,
        cx: &Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        let view = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new(crate::locale::t(cx, "dbwork.split.reset"))
                .icon(IconName::Redo)
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.navigation.top_height = 340.;
                        this.navigation.top_ratio = 0.;
                        this.sidebar_split =
                            cx.new(|_| gpui_component::resizable::ResizableState::default());
                        this.persist_navigation();
                        cx.notify();
                    });
                }),
        )
    }
    pub fn navigation_tools(&self, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let mode = self.navigation.mode;
        let can_add = mode == GroupMode::Environment;
        let recent = self
            .navigation
            .recent
            .iter()
            .filter_map(|id| self.profiles.iter().find(|p| p.id == *id))
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        let history = self.history.clone();
        let recent_view = view.clone();
        let import_view = view.clone();
        div()
            .h_flex()
            .gap_1()
            .child(
                Button::new("new-database-project")
                    .ghost()
                    .icon(gpui_component::Icon::default().path("icons/project-add.svg"))
                    .tooltip(crate::locale::t(cx, "dbwork.project.add"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_category_for(GroupMode::Group, window, cx);
                    })),
            )
            .child(
                Button::new("open-recent-connections")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .tooltip(crate::locale::t(cx, "dbwork.recent.open"))
                    .dropdown_menu(move |mut menu, _, cx| {
                        if recent.is_empty() {
                            return menu.item(
                                PopupMenuItem::new(crate::locale::t(cx, "dbwork.recent.empty"))
                                    .disabled(true),
                            );
                        }
                        for profile in &recent {
                            let view = recent_view.clone();
                            let id = profile.id;
                            menu = menu.item(
                                PopupMenuItem::element({
                                    let name = profile.name.clone();
                                    let address = connection_address(profile);
                                    let used =
                                        history.get(&id).and_then(|h| h.last_used_at.clone());
                                    move |_, cx| {
                                        div()
                                            .h_flex()
                                            .gap_3()
                                            .child(name.clone())
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(address.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(relative_used(used.as_deref(), cx)),
                                            )
                                    }
                                })
                                .icon(type_icon(profile.kind))
                                .on_click(move |_, window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if this.sessions.contains_key(&id) {
                                            this.follow_connection(id, false, cx);
                                        } else {
                                            this.connect_saved(id, window, cx);
                                        }
                                    });
                                }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new("import-connections")
                    .ghost()
                    .icon(gpui_component::Icon::default().path("icons/import.svg"))
                    .tooltip(crate::locale::t(cx, "dbwork.connections.import"))
                    .dropdown_menu(move |mut menu, _, cx| {
                        for (key, source) in [
                            ("dbwork.import.navicat", true),
                            ("dbwork.import.dbeaver", false),
                        ] {
                            let view = import_view.clone();
                            menu =
                                menu.item(PopupMenuItem::new(crate::locale::t(cx, key)).on_click(
                                    move |_, window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.import_connections(source, window, cx)
                                        });
                                    },
                                ));
                        }
                        menu
                    }),
            )
            .child(
                Button::new("connection-grouping")
                    .ghost()
                    .icon(gpui_component::Icon::default().path("icons/group-list.svg"))
                    .tooltip(crate::locale::t(cx, self.navigation.mode.key()))
                    .dropdown_menu(move |mut menu, _, cx| {
                        for mode in [
                            GroupMode::Group,
                            GroupMode::Environment,
                            GroupMode::Date,
                            GroupMode::UsedDate,
                        ] {
                            let v = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(crate::locale::t(cx, mode.key())).on_click(
                                    move |_, _, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.navigation.mode = mode;
                                            this.persist_navigation();
                                            cx.notify();
                                        });
                                    },
                                ),
                            );
                        }
                        if !can_add {
                            return menu;
                        }
                        let v = view.clone();
                        menu.item(
                            PopupMenuItem::new(crate::locale::t(cx, mode.add_key())).on_click(
                                move |_, window, cx| {
                                    let _ = v.update(cx, |this, cx| this.add_category(window, cx));
                                },
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
    fn add_category(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_category_for(self.navigation.mode, window, cx);
    }
    fn add_category_for(&mut self, mode: GroupMode, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_component::WindowExt;
        if matches!(mode, GroupMode::Date | GroupMode::UsedDate) {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::locale::t(
                cx,
                if mode == GroupMode::Group {
                    "dbwork.project.name"
                } else {
                    "dbwork.environment.name"
                },
            ))
        });
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let field = input.clone();
            let view = view.clone();
            dialog
                .title(crate::locale::t(cx, mode.add_key()))
                .child(Input::new(&input).cleanable(true))
                .child(
                    feedback::actions().child(feedback::close_button(cx)).child(
                        Button::new("save-category")
                            .icon(IconName::Check)
                            .primary()
                            .label(crate::locale::t(cx, "dbwork.save"))
                            .on_click(move |_, window, cx| {
                                let name = field.read(cx).value().trim().to_string();
                                if name.is_empty() || name.chars().count() > 80 {
                                    return;
                                }
                                let _ = view.update(cx, |this, cx| {
                                    let list = if mode == GroupMode::Group {
                                        &mut this.navigation.categories
                                    } else {
                                        &mut this.navigation.environments
                                    };
                                    if !list.contains(&name) {
                                        list.push(name.clone());
                                    }
                                    if mode == GroupMode::Group {
                                        this.navigation.mode = GroupMode::Group;
                                    }
                                    this.persist_navigation();
                                    cx.notify();
                                });
                                window.close_dialog(cx);
                                feedback::success(
                                    crate::locale::t(
                                        cx,
                                        if mode == GroupMode::Group {
                                            "dbwork.project.added"
                                        } else {
                                            "dbwork.environment.added"
                                        },
                                    ),
                                    cx,
                                );
                            }),
                    ),
                )
        });
    }
    fn move_group(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let mut names = self.ordered_groups();
        if let (Some(a), Some(b)) = (
            names.iter().position(|s| s == from),
            names.iter().position(|s| s == to),
        ) {
            let item = names.remove(a);
            names.insert(b, item);
            if self.navigation.mode == GroupMode::Environment {
                self.navigation.environment_order = names;
            } else {
                self.navigation.order = names;
            }
            self.persist_navigation();
            cx.notify();
        }
    }
    pub(super) fn render_type_picker(&self, cx: &Context<Self>) -> AnyElement {
        let filter = self.text("type_filter", cx).to_lowercase();
        let kinds = DatabaseKind::ALL
            .into_iter()
            .filter(|kind| kind.label().to_lowercase().contains(&filter))
            .collect::<Vec<_>>();
        div()
            .v_flex()
            .size_full()
            .p_4()
            .gap_4()
            .child(
                div()
                    .text_lg()
                    .child(crate::locale::t(cx, "grid.connection.choose")),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Input::new(&self.fields["type_filter"])
                            .cleanable(true)
                            .flex_1(),
                    )
                    .children(
                        [
                            (true, IconName::LayoutDashboard, "grid.view.icons"),
                            (false, IconName::Menu, "grid.view.list"),
                        ]
                        .into_iter()
                        .map(|(grid, icon, key)| {
                            Button::new(key)
                                .ghost()
                                .icon(icon)
                                .selected(self.type_grid == grid)
                                .tooltip(crate::locale::t(cx, key))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.type_grid = grid;
                                    cx.notify();
                                }))
                        }),
                    ),
            )
            .when(kinds.is_empty(), |el| {
                el.child(crate::locale::t(cx, "grid.filter.empty"))
            })
            .child(
                div()
                    .id("database-types")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .gap_3()
                    .when(self.type_grid, |el| el.flex_wrap().content_start())
                    .when(!self.type_grid, |el| el.flex_col())
                    .children(kinds.into_iter().map(|kind| {
                        Button::new(SharedString::from(format!("pick-{}", kind.label())))
                            .when(self.type_grid, |el| el.w(px(144.)).h(px(92.)).flex_col())
                            .when(!self.type_grid, |el| el.w_full().justify_start())
                            .icon(type_icon(kind))
                            .label(kind.label())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_type(kind, window, cx)
                            }))
                    })),
            )
            .into_any_element()
    }
    fn choose_type(&mut self, kind: DatabaseKind, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(profile) = self.raw_draft(cx) {
            let password = self.fields["password"].read(cx).value().to_string();
            self.typed_drafts.insert(profile.kind, (profile, password));
        }
        let common = self.editing.clone();
        let (mut profile, password) = self
            .typed_drafts
            .get(&kind)
            .cloned()
            .unwrap_or((Self::empty_profile(kind), String::new()));
        profile.group = self.text("group", cx);
        profile.environment = self.text("environment", cx);
        let name = self.text("name", cx);
        profile.name = name;
        if common.kind == kind {
            profile.id = common.id;
        }
        self.fill(profile, window, cx);
        self.baseline = Self::empty_profile(kind);
        self.baseline.id = self.editing.id;
        self.fields["password"].update(cx, |input, cx| input.set_value(password, window, cx));
        self.tab = 0;
    }
    pub(super) fn browse_sqlite(
        &mut self,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.editing.id;
        if create {
            let directory = self.file.parent().unwrap_or(Path::new(".")).to_path_buf();
            let selected = cx.prompt_for_new_path(&directory, Some("database.db"));
            cx.spawn_in(window, async move |view, cx| {
                if let Ok(Ok(Some(path))) = selected.await {
                    let work = cx.background_executor().spawn(async move {
                        turbodbn_services::database::create_sqlite_file_sync(&path)?;
                        Ok::<_, String>(path)
                    });
                    let result = work.await;
                    let _ = cx.update(|window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            match result {
                                Ok(path) => {
                                    if this.editing.id == id {
                                        this.fields["sqlite_path"].update(cx, |input, cx| {
                                            input.set_value(path.display().to_string(), window, cx)
                                        });
                                    }
                                    this.status = "SQLite 文件已创建".into();
                                }
                                Err(e) => this.status = e,
                            };
                            cx.notify();
                        });
                    });
                }
            })
            .detach();
        } else {
            let selected = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择 SQLite 数据库文件".into()),
            });
            cx.spawn_in(window, async move |view, cx| {
                if let Ok(Ok(Some(paths))) = selected.await
                    && let Some(path) = paths.first()
                {
                    let _ = cx.update(|window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            if this.editing.id == id {
                                this.fields["sqlite_path"].update(cx, |input, cx| {
                                    input.set_value(path.display().to_string(), window, cx)
                                });
                                cx.notify();
                            }
                        });
                    });
                }
            })
            .detach();
        }
    }

    pub fn navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        use gpui_component::resizable::{resizable_panel, v_resizable};
        let search = self.search.read(cx).value().trim().to_lowercase();
        let mut sections = Vec::new();
        for name in self.ordered_groups() {
            let members = self
                .profiles
                .iter()
                .filter(|p| self.group_key(p) == name)
                .cloned()
                .collect();
            sections.push(self.connection_section(
                &name,
                name.clone(),
                members,
                false,
                &search,
                cx,
            ));
        }
        let top = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(
                Button::new("new-connection")
                    .icon(IconName::Plus)
                    .label(crate::locale::t(cx, "dbwork.connection.new"))
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.dirty(cx) {
                            this.status = "请先保存或取消当前修改".into();
                            cx.emit(OpenDatabase);
                            cx.notify();
                            return;
                        }
                        this.typed_drafts.clear();
                        this.fill(Self::empty_profile(DatabaseKind::MySql), window, cx);
                        this.choosing_type = true;
                        cx.emit(OpenDatabase);
                        cx.notify();
                    })),
            )
            .child(Input::new(&self.search).cleanable(true))
            .child(
                div()
                    .id("connection-sections")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(sections),
            );
        let bottom = self.render_object_tree(window, cx);
        let source = cx.entity().downgrade();
        v_resizable("database-vertical-split")
            .with_state(&self.sidebar_split)
            .on_resize(move |state, _, cx| {
                let sizes = state.read(cx).sizes();
                let total = sizes.iter().map(|p| f32::from(*p)).sum::<f32>();
                let height = sizes.first().copied().map(f32::from);
                if let Some(h) = height {
                    let _ = source.update(cx, |this, _| {
                        this.navigation.top_height = h;
                        if total > 0. {
                            this.navigation.top_ratio = (h / total).clamp(0.15, 0.85);
                        }
                        this.persist_navigation();
                    });
                }
            })
            .child(
                resizable_panel()
                    .size(px(if self.navigation.top_ratio > 0. {
                        (f32::from(window.viewport_size().height) - 100.)
                            * self.navigation.top_ratio.clamp(0.15, 0.85)
                    } else if self.navigation.top_height > 0. {
                        self.navigation.top_height
                    } else {
                        340.
                    }))
                    .size_range(px(150.)..px(2000.))
                    .child(top),
            )
            .child(
                resizable_panel()
                    .size_range(px(180.)..px(4000.))
                    .child(bottom),
            )
            .into_any_element()
    }
    fn connection_section(
        &self,
        key: &str,
        label: String,
        profiles: Vec<Profile>,
        recent: bool,
        search: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label = match label.as_str() {
            "未分组" => crate::locale::t(cx, "dbwork.ungrouped").to_string(),
            "未设置环境" => crate::locale::t(cx, "dbwork.environment.unset").to_string(),
            "日期未知" => crate::locale::t(cx, "dbwork.date.unknown").to_string(),
            _ => label,
        };
        let stored_key = if self.navigation.mode == GroupMode::Environment {
            format!("environment:{key}")
        } else {
            key.to_string()
        };
        let collapsed = self.navigation.collapsed.contains(&stored_key) && search.is_empty();
        let toggle = stored_key;
        let drag = key.to_string();
        let target = key.to_string();
        let mut header = div()
            .h_flex()
            .h(px(30.))
            .gap_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(label),
            )
            .child(
                Button::new(SharedString::from(format!("fold-{key}")))
                    .ghost()
                    .icon(if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.navigation.collapsed.remove(&toggle) {
                            this.navigation.collapsed.insert(toggle.clone());
                        }
                        this.persist_navigation();
                        cx.notify();
                    })),
            );
        if !recent && !matches!(self.navigation.mode, GroupMode::Date | GroupMode::UsedDate) {
            header = header.child(
                div()
                    .id(SharedString::from(format!("grip-{key}")))
                    .w(px(24.))
                    .h(px(28.))
                    .flex_shrink_0()
                    .h_flex()
                    .justify_center()
                    .cursor_grab()
                    .child(gpui_component::Icon::new(IconName::Menu))
                    .on_drag(GroupDrag(drag), |drag, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| drag.clone())
                    }),
            );
        }
        let mut block = div()
            .id(SharedString::from(format!("connection-section-{key}")))
            .v_flex()
            .on_drop(cx.listener(move |this, source: &GroupDrag, _, cx| {
                if !recent {
                    this.move_group(&source.0, &target, cx)
                }
            }))
            .child(header);
        if !collapsed {
            for p in profiles {
                let haystack = format!(
                    "{} {} {} {} {} {} {}",
                    p.name,
                    p.host,
                    p.database,
                    p.environment,
                    p.kind.label(),
                    p.group,
                    self.catalog(&p).join(" ")
                )
                .to_lowercase();
                if !search.is_empty() && !haystack.contains(search) {
                    continue;
                }
                let id = p.id;
                let connected = self.states.get(&id).is_some_and(|s| s == "已连接");
                let state = self
                    .states
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "未连接".into());
                let label = if p.name.trim().is_empty() {
                    crate::locale::t(cx, "dbwork.connection.name.required").to_string()
                } else {
                    p.name.clone()
                };
                let address = connection_address(&p);
                let pending = state.ends_with("中…");
                let failed = state.contains("失败") || state.contains("错误");
                let selected = self
                    .selected_database
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id);
                let scope = key.to_string();
                let menu_view = cx.entity().downgrade();
                block = block.child(
                    div()
                        .h_flex()
                        .items_center()
                        .h(px(32.))
                        .flex_shrink_0()
                        .gap_1()
                        .child(
                            div()
                                .w(px(4.))
                                .h(px(18.))
                                .rounded_sm()
                                .bg(environment_color(&p.environment)),
                        )
                        .child(
                            div().flex_1().min_w_0().child(crate::context_menu::scoped(
                                SharedString::from(format!("connection-menu-{scope}-{id}")),
                                Button::new(SharedString::from(format!("connection-{scope}-{id}")))
                                    .ghost()
                                    .icon(type_icon(p.kind))
                                    .child(
                                        div()
                                            .max_w(relative(0.55))
                                            .flex_shrink_0()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(label.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(address.clone()),
                                    )
                                    .overflow_hidden()
                                    .tooltip(format!(
                                        "{label} · {address} · {} · {}",
                                        p.environment,
                                        p.kind.label()
                                    ))
                                    .justify_start()
                                    .selected(selected)
                                    .w_full()
                                    .min_w_0()
                                    .on_click(cx.listener(
                                        move |this, event: &ClickEvent, window, cx| {
                                            if event.click_count() == 2
                                                && this
                                                    .states
                                                    .get(&id)
                                                    .is_none_or(|s| s != "已连接")
                                            {
                                                this.connect_saved(id, window, cx);
                                            } else {
                                                this.follow_connection(id, false, cx);
                                            }
                                        },
                                    )),
                                move |menu, _, cx| {
                                    if let Some(view) = menu_view.upgrade() {
                                        view.update(cx, |this, cx| {
                                            this.connection_menu(menu, id, false, cx)
                                        })
                                    } else {
                                        menu
                                    }
                                },
                            )),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("status-dot-{scope}-{id}")))
                                .w(px(20.))
                                .h(px(32.))
                                .h_flex()
                                .justify_center()
                                .child(if pending {
                                    gpui_component::Icon::new(IconName::LoaderCircle)
                                        .into_any_element()
                                } else {
                                    div()
                                        .w(px(7.))
                                        .h(px(7.))
                                        .rounded_full()
                                        .bg(if connected {
                                            hsla(0.4, 0.65, 0.48, 1.)
                                        } else if failed {
                                            cx.theme().danger
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .into_any_element()
                                })
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(state.clone())
                                        .build(window, cx)
                                }),
                        )
                        .child(
                            div()
                                .w(px(28.))
                                .flex_shrink_0()
                                .when(connected || pending, |el| {
                                    el.child(
                                        Button::new(SharedString::from(format!(
                                            "disconnect-{scope}-{id}"
                                        )))
                                        .ghost()
                                        .icon(IconName::Close)
                                        .tooltip(crate::locale::t(
                                            cx,
                                            if pending {
                                                "dbwork.connection.cancel"
                                            } else {
                                                "dbwork.connection.disconnect"
                                            },
                                        ))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.disconnect(id, cx);
                                                feedback::success(
                                                    crate::locale::t(
                                                        cx,
                                                        "feedback.connection.disconnected",
                                                    ),
                                                    cx,
                                                );
                                            }),
                                        ),
                                    )
                                }),
                        ),
                );
            }
        }
        block.into_any_element()
    }
}
fn connection_address(p: &Profile) -> String {
    if p.kind == DatabaseKind::Sqlite {
        return Path::new(&p.sqlite_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    format!("{}:{}/{}", p.host, p.port, p.database)
}

fn relative_used(at: Option<&str>, cx: &App) -> SharedString {
    let Some(at) = at.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) else {
        return crate::locale::t(cx, "dbwork.recent.unknown");
    };
    let seconds = (chrono::Utc::now() - at.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    let (key, n) = if seconds < 60 {
        ("dbwork.recent.now", 0)
    } else if seconds < 3600 {
        ("dbwork.recent.minutes", seconds / 60)
    } else if seconds < 86400 {
        ("dbwork.recent.hours", seconds / 3600)
    } else if seconds < 2592000 {
        ("dbwork.recent.days", seconds / 86400)
    } else if seconds < 31536000 {
        ("dbwork.recent.months", seconds / 2592000)
    } else {
        ("dbwork.recent.years", seconds / 31536000)
    };
    if n == 0 {
        crate::locale::t(cx, key)
    } else {
        crate::locale::tr(cx, key, &[("n", &n.to_string())])
    }
}
