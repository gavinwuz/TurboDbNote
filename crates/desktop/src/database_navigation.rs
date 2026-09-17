use super::*;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Navigation {
    pub order: Vec<String>,
    pub collapsed: std::collections::HashSet<String>,
}
pub(super) fn load(path: &Path) -> Result<Navigation, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Navigation::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("导航文件过大".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "导航文件格式无效".into())
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
            let result = (|| -> Result<(), String> {
                let parent = path.parent().ok_or("导航路径无效")?;
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                let mut file =
                    tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
                file.write_all(&serde_json::to_vec(&state).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                file.as_file().sync_all().map_err(|e| e.to_string())?;
                file.persist(&path).map_err(|e| e.to_string())?;
                Ok(())
            })();
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
fn group(p: &Profile) -> String {
    if p.group.trim().is_empty() {
        "未分组".into()
    } else {
        p.group.clone()
    }
}
fn environment_color(name: &str) -> Hsla {
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
fn type_icon(kind: DatabaseKind) -> gpui_component::Icon {
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
    fn persist_navigation(&self) {
        let _ = self.nav_writer.force_send(self.navigation.clone());
    }
    fn ordered_groups(&self) -> Vec<String> {
        let mut names = self.profiles.iter().map(group).collect::<Vec<_>>();
        names.sort();
        names.dedup();
        let mut ordered = self
            .navigation
            .order
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
    fn move_group(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let mut names = self.ordered_groups();
        if let (Some(a), Some(b)) = (
            names.iter().position(|s| s == from),
            names.iter().position(|s| s == to),
        ) {
            let item = names.remove(a);
            names.insert(b, item);
            self.navigation.order = names;
            self.persist_navigation();
            cx.notify();
        }
    }
    pub(super) fn render_type_picker(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .v_flex()
            .size_full()
            .p_4()
            .gap_4()
            .child(div().text_lg().child("新建连接 · 选择数据库类型"))
            .child("选择类型后填写连接信息，可随时返回切换。")
            .children(DatabaseKind::ALL.into_iter().map(|kind| {
                Button::new(SharedString::from(format!("pick-{}", kind.label())))
                    .icon(type_icon(kind))
                    .label(kind.label())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.choose_type(kind, window, cx)),
                    )
            }))
            .into_any_element()
    }
    fn choose_type(&mut self, kind: DatabaseKind, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(profile) = self.draft(cx) {
            let password = self.fields["password"].read(cx).value().to_string();
            self.typed_drafts.insert(profile.kind, (profile, password));
        }
        let common = self.editing.clone();
        let (mut profile, password) = self
            .typed_drafts
            .get(&kind)
            .cloned()
            .unwrap_or((turbodbn_services::database::defaults(kind), String::new()));
        profile.group = self.text("group", cx);
        profile.environment = self.text("environment", cx);
        let name = self.text("name", cx);
        if !name.starts_with("新建 ") {
            profile.name = name;
        }
        if common.kind == kind {
            profile.id = common.id;
        }
        self.fill(profile, window, cx);
        self.baseline = turbodbn_services::database::defaults(kind);
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
        let search = self.search.read(cx).value().trim().to_lowercase();
        let groups = self.ordered_groups();
        let mut rows = Vec::new();
        for (index, name) in groups.iter().enumerate() {
            let members = self
                .profiles
                .iter()
                .filter(|p| group(p) == *name)
                .cloned()
                .collect::<Vec<_>>();
            let matches = members.iter().any(|p| {
                format!(
                    "{} {} {} {} {}",
                    p.name,
                    p.host,
                    p.database,
                    p.kind.label(),
                    self.catalog(p).join(" ")
                )
                .to_lowercase()
                .contains(&search)
            });
            if !search.is_empty() && !name.to_lowercase().contains(&search) && !matches {
                continue;
            }
            let collapsed = self.navigation.collapsed.contains(name) && search.is_empty();
            let toggle = name.clone();
            let target = name.clone();
            let drag = name.clone();
            let view = cx.entity().downgrade();
            let menu_name = name.clone();
            let order = groups.clone();
            let header = div()
                .id(SharedString::from(format!("group-{name}")))
                .h_flex()
                .gap_1()
                .on_drag(GroupDrag(drag.clone()), |drag, _, _, cx| {
                    cx.new(|_| drag.clone())
                })
                .on_drop(cx.listener(move |this, source: &GroupDrag, _, cx| {
                    this.move_group(&source.0, &target, cx)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("grip-{name}")))
                        .w(px(16.))
                        .h(px(32.))
                        .h_flex()
                        .cursor_grab()
                        .child(gpui_component::Icon::new(IconName::Menu))
                        .on_drag(GroupDrag(drag), |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| drag.clone())
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("toggle-{name}")))
                        .ghost()
                        .icon(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .label(name.clone())
                        .justify_start()
                        .flex_1()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.navigation.collapsed.remove(&toggle) {
                                this.navigation.collapsed.insert(toggle.clone());
                            }
                            this.persist_navigation();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("menu-{name}")))
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .tooltip("调整分组顺序")
                        .dropdown_menu(move |menu, _, _| {
                            let up = view.clone();
                            let down = view.clone();
                            let name_up = menu_name.clone();
                            let name_down = menu_name.clone();
                            let previous = order.get(index.wrapping_sub(1)).cloned();
                            let next = order.get(index + 1).cloned();
                            menu.item(
                                PopupMenuItem::new("上移")
                                    .disabled(previous.is_none())
                                    .on_click(move |_, _, cx| {
                                        if let Some(to) = &previous {
                                            let _ = up.update(cx, |this, cx| {
                                                this.move_group(&name_up, to, cx)
                                            });
                                        }
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("下移")
                                    .disabled(next.is_none())
                                    .on_click(move |_, _, cx| {
                                        if let Some(to) = &next {
                                            let _ = down.update(cx, |this, cx| {
                                                this.move_group(&name_down, to, cx)
                                            });
                                        }
                                    }),
                            )
                        }),
                );
            let mut block = div().v_flex().gap_2().child(header);
            if !collapsed {
                for profile in members {
                    let id = profile.id;
                    let state = self.states.get(&id).cloned().unwrap_or("未连接".into());
                    let connected = state == "已连接";
                    let mut names = if !profile.database.is_empty() {
                        vec![profile.database.clone()]
                    } else {
                        self.catalog(&profile)
                    };
                    if !search.is_empty()
                        && !format!(
                            "{} {} {} {}",
                            name,
                            profile.name,
                            profile.host,
                            profile.kind.label()
                        )
                        .to_lowercase()
                        .contains(&search)
                    {
                        names.retain(|n| n.to_lowercase().contains(&search));
                        if names.is_empty() {
                            continue;
                        }
                    }
                    let expanded = !self.collapsed.contains(&id);
                    let status_icon = if connected {
                        IconName::CircleCheck
                    } else if state.ends_with("中…") {
                        IconName::LoaderCircle
                    } else if state.contains("失败") {
                        IconName::CircleX
                    } else {
                        IconName::Dash
                    };
                    let environment = profile.environment.clone();
                    let mut item = div()
                        .v_flex()
                        .gap_1()
                        .pl_2()
                        .child(
                            div()
                                .h_flex()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!("connection-{id}")))
                                        .ghost()
                                        .icon(type_icon(profile.kind))
                                        .label(profile.name.clone())
                                        .justify_start()
                                        .tooltip(format!(
                                            "{} · {}",
                                            profile.kind.label(),
                                            profile.name
                                        ))
                                        .flex_1()
                                        .min_w_0()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.select(id, window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .px_1()
                                        .rounded_sm()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .h_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w(px(6.))
                                                .h(px(6.))
                                                .rounded_full()
                                                .bg(environment_color(&environment)),
                                        )
                                        .child(environment),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("state-{id}")))
                                        .ghost()
                                        .icon(status_icon)
                                        .tooltip(state.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if profile.kind == DatabaseKind::Sqlite {
                                    format!(
                                        "{} · {state}",
                                        Path::new(&profile.sqlite_path)
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                    )
                                } else {
                                    format!("{}:{} · {state}", profile.host, profile.port)
                                }),
                        )
                        .child(
                            div()
                                .h_flex()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!("expand-{id}")))
                                        .ghost()
                                        .icon(if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .label(format!("数据库 · {}", names.len()))
                                        .flex_1()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.collapsed.remove(&id) {
                                                this.collapsed.insert(id);
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("refresh-{id}")))
                                        .ghost()
                                        .icon(IconName::Redo)
                                        .tooltip("刷新数据库")
                                        .disabled(!connected || self.active_operation.is_some())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.request_operation(
                                                id,
                                                OperationKind::Databases,
                                                cx,
                                            );
                                        })),
                                )
                                .when(profile.kind.can_create_database(), |el| {
                                    el.child(
                                        Button::new(SharedString::from(format!("create-{id}")))
                                            .ghost()
                                            .icon(IconName::Plus)
                                            .tooltip("创建数据库")
                                            .disabled(!connected || self.active_operation.is_some())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.create_database_dialog(id, window, cx)
                                            })),
                                    )
                                }),
                        );
                    if expanded {
                        if names.len() > 20 {
                            if let std::collections::hash_map::Entry::Vacant(entry) =
                                self.db_filters.entry(id)
                            {
                                let filter = cx.new(|cx| {
                                    InputState::new(window, cx).placeholder("筛选数据库")
                                });
                                let sub = cx.observe(&filter, |_, _, cx| cx.notify());
                                entry.insert(filter);
                                self.db_filter_subscriptions.insert(id, sub);
                            }
                            let filter = self.db_filters[&id].clone();
                            let query = filter.read(cx).value().trim().to_lowercase();
                            names.retain(|n| n.to_lowercase().contains(&query));
                            item = item.child(Input::new(&filter));
                        }
                        let total = names.len();
                        let limit = *self.db_limits.entry(id).or_insert(20);
                        names.truncate(limit);
                        let count = names.len();
                        let kind = profile.kind;
                        let scroll = self.db_scrolls.entry(id).or_default().clone();
                        if count > 0 {
                            item = item.child(
                                uniform_list(
                                    SharedString::from(format!("database-list-{id}")),
                                    count,
                                    cx.processor(
                                        move |this, range: std::ops::Range<usize>, _, cx| {
                                            range
                                                .map(|ix| {
                                                    let database = names[ix].clone();
                                                    let selected = this.selected_database.as_ref()
                                                        == Some(&(id, database.clone()));
                                                    Button::new(("database-row", ix))
                                                        .ghost()
                                                        .icon(
                                                            gpui_component::Icon::default()
                                                                .path("icons/database.svg"),
                                                        )
                                                        .label(if kind == DatabaseKind::Redis {
                                                            format!("DB {database}")
                                                        } else {
                                                            database.clone()
                                                        })
                                                        .selected(selected)
                                                        .justify_start()
                                                        .tooltip(database.clone())
                                                        .disabled(
                                                            !connected
                                                                || this.active_operation.is_some(),
                                                        )
                                                        .w_full()
                                                        .h(px(28.))
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.select_database(
                                                                    id,
                                                                    database.clone(),
                                                                    cx,
                                                                )
                                                            },
                                                        ))
                                                        .into_any_element()
                                                })
                                                .collect::<Vec<_>>()
                                        },
                                    ),
                                )
                                .track_scroll(scroll)
                                .w_full()
                                .h(px((count.min(8) * 28) as f32)),
                            );
                        } else {
                            item = item.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if connected {
                                        "无匹配数据库"
                                    } else {
                                        "连接后加载；搜索不会自动连接"
                                    }),
                            );
                        }
                        if total > limit {
                            item = item.child(
                                Button::new(SharedString::from(format!("more-databases-{id}")))
                                    .ghost()
                                    .label(format!("查看更多（剩余 {}）", total - limit))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        *this.db_limits.entry(id).or_insert(20) += 50;
                                        cx.notify();
                                    })),
                            );
                        }
                    }
                    block = block.child(item);
                }
            }
            rows.push(block);
        }
        div()
            .v_flex()
            .gap_3()
            .w_full()
            .child(
                Button::new("new-connection")
                    .icon(IconName::Plus)
                    .label("新建连接")
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.dirty(cx) {
                            this.status = "请先保存或取消当前修改".into();
                            cx.emit(OpenDatabase);
                            cx.notify();
                            return;
                        }
                        this.typed_drafts.clear();
                        this.fill(Profile::default(), window, cx);
                        this.choosing_type = true;
                        cx.emit(OpenDatabase);
                        cx.notify();
                    })),
            )
            .child(Input::new(&self.search))
            .children(rows)
            .into_any_element()
    }
}
