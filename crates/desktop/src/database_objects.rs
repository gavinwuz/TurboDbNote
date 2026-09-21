use super::*;
use gpui_component::WindowExt;
impl DatabaseView {
    pub(super) fn clear_object_view(&mut self) {
        self.object_scroll = UniformListScrollHandle::new();
        self.selected_object = None;
        self.object_items.clear();
        self.object_details.clear();
        self.object_cursor = None;
        self.catalog_objects.clear();
        self.schemas.clear();
        self.pending_object_action = None;
        self.expanded_objects.clear();
    }
    pub(super) fn follow_connection(
        &mut self,
        id: Uuid,
        refresh: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !refresh
            && self
                .selected_database
                .as_ref()
                .is_some_and(|(current, _)| *current == id)
        {
            return true;
        }
        let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() else {
            return false;
        };
        if let Some((_, _, kind)) = &self.active_operation {
            if self.workbench_target.is_some() || !is_metadata_read(kind) {
                feedback::error(crate::locale::t(cx, "dbwork.switch.busy"), cx);
                return false;
            }
            // Metadata requests are read-only. Retire the request ID without disconnecting;
            // its late result must not populate the newly selected connection's object tree.
            self.active_operation = None;
        }
        let names = self.catalog(&profile);
        let database = self
            .navigation
            .databases
            .get(&id)
            .filter(|db| names.contains(db))
            .cloned()
            .or_else(|| names.first().cloned())
            .unwrap_or_else(|| {
                if profile.kind == DatabaseKind::Sqlite {
                    std::path::Path::new(&profile.sqlite_path)
                        .file_name()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    profile.database.clone()
                }
            });
        self.selected_database = Some((id, database.clone()));
        self.clear_object_view();
        if self.states.get(&id).is_some_and(|s| s == "已连接") {
            self.select_database(id, database, cx);
        } else {
            self.object_notice = crate::locale::t(cx, "dbwork.switch.disconnected").to_string();
            cx.notify();
        }
        true
    }
    pub(super) fn request_operation(
        &mut self,
        id: Uuid,
        kind: OperationKind,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_operation.is_some() {
            self.object_notice = "请等待当前操作完成；断开可取消".into();
            feedback::error(self.object_notice.clone(), cx);
            cx.notify();
            return false;
        }
        let Some(session) = self.sessions.get(&id) else {
            self.object_notice = "请先连接数据库".into();
            feedback::error(self.object_notice.clone(), cx);
            cx.notify();
            return false;
        };
        if self.states.get(&id).is_none_or(|s| s != "已连接") {
            return false;
        }
        self.request_sequence += 1;
        let request = self.request_sequence;
        if let Err(error) = session.request(Operation {
            id: request,
            kind: kind.clone(),
        }) {
            self.object_notice = error;
            cx.notify();
            return false;
        }
        if let OperationKind::Details { object, .. } = &kind {
            self.selected_object = Some(object.clone());
        }
        if let OperationKind::Workbench(request) = &kind
            && let Action::Describe(object) = &request.action
        {
            self.selected_object = Some(ObjectItem::new(
                object.schema.clone(),
                object.name.clone(),
                object.kind.clone(),
            ));
        }
        self.active_operation = Some((id, request, kind));
        self.object_notice = "正在读取 / 执行…".into();
        cx.notify();
        true
    }
    pub(super) fn select_database(&mut self, id: Uuid, database: String, cx: &mut Context<Self>) {
        if self.request_operation(
            id,
            if self
                .profiles
                .iter()
                .any(|p| p.id == id && p.kind == DatabaseKind::Redis)
            {
                OperationKind::Objects {
                    database: database.clone(),
                    cursor: None,
                }
            } else {
                OperationKind::Workbench(Box::new(workbench::Request {
                    database: database.clone(),
                    action: Action::Catalog,
                }))
            },
            cx,
        ) {
            self.navigation.databases.insert(id, database.clone());
            self.selected_database = Some((id, database));
            self.clear_object_view();
            cx.notify();
        }
    }
    pub(super) fn operation_result(
        &mut self,
        id: Uuid,
        request: u64,
        result: Result<OperationResult, String>,
        cx: &mut Context<Self>,
    ) {
        let Some((connection, active, kind)) = self.active_operation.clone() else {
            return;
        };
        if connection != id || active != request {
            return;
        }
        self.active_operation = None;
        if result.is_ok() {
            self.remember_connection(id, cx);
        }
        if let Some(target) = self.workbench_target.take() {
            let result = result.and_then(|r| match r {
                OperationResult::Workbench(r) => Ok(*r),
                _ => Err("响应类型不匹配".into()),
            });
            let _ = target.update(cx, |view, cx| view.receive(result, cx));
            self.object_notice = format!("{} 个对象", self.catalog_objects.len());
            cx.notify();
            return;
        }
        match result {
            Ok(OperationResult::Workbench(reply)) => match *reply {
                Reply::Catalog { objects, notice } => {
                    self.catalog_objects = objects;
                    self.object_notice = if notice.is_empty() {
                        format!("{} 个对象", self.catalog_objects.len())
                    } else {
                        notice
                    };
                }
                Reply::Schema(mut schema) => {
                    if let OperationKind::Workbench(request) = kind
                        && let Action::Describe(object) = request.action
                    {
                        self.object_details = schema
                            .columns
                            .iter()
                            .map(|c| (c.name.clone(), format!("{} · {}", c.data_type, c.comment)))
                            .collect();
                        self.object_details.push(("DDL".into(), schema.ddl.clone()));
                        if schema.triggers.values.is_empty() {
                            schema.triggers.values = self
                                .catalog_objects
                                .iter()
                                .filter(|o| {
                                    o.kind == "trigger"
                                        && o.parent == object.name
                                        && o.schema == object.schema
                                })
                                .map(|o| vec![Some(o.name.clone())])
                                .collect();
                        }
                        self.schemas.insert(Self::object_key(&object), schema);
                    }
                    self.object_notice = "结构已更新".into();
                }
                Reply::Executed => {
                    self.object_notice = "SQL 已执行，请刷新对象列表".into();
                    feedback::success(crate::locale::t(cx, "dbwork.executed"), cx);
                    self.schemas.clear();
                }
                Reply::Page { .. } => {}
            },
            Err(error) => {
                self.pending_object_action = None;
                self.object_notice = error.clone();
                feedback::error(error.clone(), cx);
                self.status = error;
            }
            Ok(OperationResult::Databases(names)) => {
                let count = names.len();
                if let Some(info) = self.info.get_mut(&id) {
                    info.databases = names;
                }
                self.object_notice = format!("已刷新 {count} 个数据库");
            }
            Ok(OperationResult::Objects {
                items,
                cursor,
                notice,
            }) => {
                if matches!(
                    kind,
                    OperationKind::Objects {
                        cursor: Some(_),
                        ..
                    }
                ) {
                    for item in items {
                        if !self.object_items.iter().any(|old| {
                            old.schema == item.schema
                                && old.name == item.name
                                && old.key == item.key
                        }) {
                            self.object_items.push(item);
                        }
                    }
                } else {
                    self.object_items = items;
                }
                self.object_cursor = cursor;
                self.object_notice =
                    notice.unwrap_or_else(|| format!("{} 个对象", self.object_items.len()));
            }
            Ok(OperationResult::Details(details)) => {
                self.detail_scroll = UniformListScrollHandle::new();
                self.object_details = details;
                self.object_notice = "对象详情已更新".into();
            }
            Ok(OperationResult::Created(name)) => {
                self.status = format!("数据库已创建：{name}");
                feedback::success(self.status.clone(), cx);
                self.object_notice = self.status.clone();
                if let Some(profile) = self.profiles.iter().find(|p| p.id == id)
                    && profile.database.is_empty()
                {
                    if let Some(info) = self.info.get_mut(&id)
                        && !info.databases.contains(&name)
                    {
                        info.databases.push(name.clone());
                        info.databases.sort();
                    }
                    self.select_database(id, name, cx);
                } else {
                    self.object_notice
                        .push_str("；当前连接限定单库，未修改连接配置");
                }
            }
        }
        cx.notify();
    }
    pub(super) fn create_database_dialog(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        if !profile.kind.can_create_database() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::locale::t(cx, "feedback.database.name"))
        });
        let view = cx.entity().downgrade();
        let generation = self.generation.get(&id).copied().unwrap_or_default();
        window.open_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let field = input.clone();
            let profile = profile.clone();
            dialog
                .title(crate::locale::t(cx, "dbwork.database.new"))
                .w(px(500.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(format!(
                            "{} · {} · {}:{}",
                            profile.name, profile.environment, profile.host, profile.port
                        ))
                        .child(Input::new(&input)),
                )
                .child(
                    feedback::actions().child(feedback::close_button(cx)).child(
                        Button::new("confirm-create-database")
                            .primary()
                            .icon(IconName::Plus)
                            .label(crate::locale::t(cx, "feedback.database.create"))
                            .on_click(move |_, window, cx| {
                                let name = field.read(cx).value().trim().to_string();
                                if let Err(error) =
                                    turbodbn_services::database::validate_name(profile.kind, &name)
                                {
                                    feedback::error(error, cx);
                                    return;
                                }
                                let view = view.clone();
                                feedback::confirm(
                                    crate::locale::t(cx, "feedback.database.confirm"),
                                    format!(
                                        "{} · {} / {}",
                                        profile.name, profile.environment, name
                                    ),
                                    window,
                                    cx,
                                    move |window, cx| {
                                        let sent = view
                                            .update(cx, |this, cx| {
                                                if this
                                                    .generation
                                                    .get(&id)
                                                    .copied()
                                                    .unwrap_or_default()
                                                    != generation
                                                {
                                                    feedback::error(
                                                        crate::locale::t(
                                                            cx,
                                                            "dbwork.context.changed",
                                                        ),
                                                        cx,
                                                    );
                                                    return false;
                                                }
                                                this.request_operation(
                                                    id,
                                                    OperationKind::CreateDatabase {
                                                        name: name.clone(),
                                                    },
                                                    cx,
                                                )
                                            })
                                            .unwrap_or(false);
                                        if sent {
                                            window.close_dialog(cx);
                                        }
                                    },
                                );
                            }),
                    ),
                )
        });
    }
    pub fn render_objects(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((id, database)) = self.selected_database.clone() else {
            return div()
                .v_flex()
                .gap_3()
                .child("数据库对象")
                .child("连接后，在左侧选择数据库以浏览表、视图或 Redis Key。列与索引在这里展示。")
                .into_any_element();
        };
        let profile = self.profiles.iter().find(|p| p.id == id).cloned();
        let busy = self.active_operation.is_some();
        let connected = self.states.get(&id).is_some_and(|s| s == "已连接");
        let reload = database.clone();
        let back = database.clone();
        let mut body = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .child(div().text_lg().child(format!(
                "{} / {}",
                profile.as_ref().map(|p| p.name.as_str()).unwrap_or("连接"),
                database
            )))
            .when(!self.object_details.is_empty(), |el| {
                el.child(
                    self.selected_object
                        .as_ref()
                        .map(|o| {
                            if o.schema.is_empty() {
                                o.name.clone()
                            } else {
                                format!("{}.{}", o.schema, o.name)
                            }
                        })
                        .unwrap_or_default(),
                )
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("refresh-objects")
                            .icon(IconName::Redo)
                            .label("刷新")
                            .disabled(busy || !connected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_database(id, reload.clone(), cx)
                            })),
                    )
                    .when(!self.object_details.is_empty(), |el| {
                        el.child(
                            Button::new("back-to-objects")
                                .icon(IconName::ArrowLeft)
                                .label("对象列表")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.object_details.clear();
                                    this.selected_object = None;
                                    this.object_notice =
                                        format!("{} · {} 个对象", back, this.object_items.len());
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if connected {
                        self.object_notice.clone()
                    } else {
                        "连接已断开，以下为缓存；重连后刷新".into()
                    }),
            );
        if self.object_details.is_empty() {
            let count = self.object_items.len();
            let db = database.clone();
            body = body
                .child(
                    uniform_list(
                        "database-objects",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|ix| {
                                    let object = this.object_items[ix].clone();
                                    let name = if object.schema.is_empty() {
                                        object.name.clone()
                                    } else {
                                        format!("{}.{}", object.schema, object.name)
                                    };
                                    let target = db.clone();
                                    Button::new(("object", ix))
                                        .ghost()
                                        .icon(if object.kind == "Key" {
                                            IconName::File
                                        } else {
                                            IconName::GalleryVerticalEnd
                                        })
                                        .label(format!("{name} · {}", object.kind))
                                        .tooltip(name)
                                        .justify_start()
                                        .h(px(32.))
                                        .w_full()
                                        .disabled(busy || !connected)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.request_operation(
                                                id,
                                                OperationKind::Details {
                                                    database: target.clone(),
                                                    object: object.clone(),
                                                },
                                                cx,
                                            );
                                        }))
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.object_scroll.clone())
                    .h(px(360.))
                    .w_full(),
                )
                .when(count == 0 && !busy, |el| el.child("当前数据库没有可见对象"));
            if let Some(cursor) = self.object_cursor.clone() {
                body = body.child(
                    Button::new("more-keys")
                        .label("继续扫描 Key")
                        .disabled(busy || !connected)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request_operation(
                                id,
                                OperationKind::Objects {
                                    database: database.clone(),
                                    cursor: Some(cursor.clone()),
                                },
                                cx,
                            );
                        })),
                );
            }
        } else {
            body = body.child(
                uniform_list(
                    "object-details",
                    self.object_details.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                        range
                            .map(|ix| {
                                let (name, value) = &this.object_details[ix];
                                div()
                                    .v_flex()
                                    .h(px(64.))
                                    .gap_1()
                                    .child(name.clone())
                                    .child(
                                        div()
                                            .text_sm()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(value.clone()),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.detail_scroll.clone())
                .h(px(460.))
                .w_full(),
            );
        }
        body.into_any_element()
    }
}

fn is_metadata_read(kind: &OperationKind) -> bool {
    match kind {
        OperationKind::Databases
        | OperationKind::Objects { .. }
        | OperationKind::Details { .. } => true,
        OperationKind::Workbench(request) => {
            matches!(request.action, Action::Catalog | Action::Describe(_))
        }
        OperationKind::CreateDatabase { .. } => false,
    }
}
