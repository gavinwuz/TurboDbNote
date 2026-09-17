use super::*;
use gpui_component::WindowExt;
impl DatabaseView {
    pub(super) fn request_operation(
        &mut self,
        id: Uuid,
        kind: OperationKind,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_operation.is_some() {
            self.object_notice = "请等待当前操作完成；断开可取消".into();
            cx.notify();
            return false;
        }
        let Some(session) = self.sessions.get(&id) else {
            self.object_notice = "请先连接数据库".into();
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
        self.active_operation = Some((id, request, kind));
        self.object_notice = "正在读取 / 执行…".into();
        cx.notify();
        true
    }
    pub(super) fn select_database(&mut self, id: Uuid, database: String, cx: &mut Context<Self>) {
        if self.request_operation(
            id,
            OperationKind::Objects {
                database: database.clone(),
                cursor: None,
            },
            cx,
        ) {
            self.selected_database = Some((id, database));
            self.object_scroll = UniformListScrollHandle::new();
            self.selected_object = None;
            self.object_items.clear();
            self.object_details.clear();
            self.object_cursor = None;
            cx.emit(OpenObjects);
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
        match result {
            Err(error) => {
                self.object_notice = error.clone();
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
                    self.db_filters.remove(&id);
                    self.db_filter_subscriptions.remove(&id);
                    if let Some(info) = self.info.get(&id) {
                        self.db_limits.insert(id, info.databases.len());
                        if let Some(index) = info.databases.iter().position(|n| n == &name) {
                            self.db_scrolls
                                .entry(id)
                                .or_default()
                                .scroll_to_item(index, ScrollStrategy::Center);
                        }
                    }
                    self.collapsed.remove(&id);
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
        self.object_notice.clear();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("新数据库名称"));
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let source = view.clone();
            let field = input.clone();
            let target = profile.clone();
            let available = view.upgrade().is_some_and(|v| {
                let this = v.read(cx);
                this.active_operation.is_none()
                    && this.states.get(&id).is_some_and(|s| s == "已连接")
            });
            dialog
                .title("创建数据库")
                .w(px(480.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(format!(
                            "{} · {} · {}",
                            profile.kind.label(),
                            profile.name,
                            profile.environment
                        ))
                        .child(format!("目标：{}:{}", profile.host, profile.port))
                        .child(Input::new(&input))
                        .when(!profile.database.is_empty(), |el| {
                            el.child("当前连接限定单库；创建不会修改原连接配置。")
                        })
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(
                                    Button::new("confirm-create-database")
                                        .primary()
                                        .icon(IconName::Plus)
                                        .label("创建")
                                        .disabled(!available)
                                        .on_click(move |_, window, cx| {
                                            let name = field.read(cx).value().trim().to_string();
                                            let sent = source
                                                .update(cx, |this, cx| {
                                                    if let Err(error) =
                                                        turbodbn_services::database::validate_name(
                                                            target.kind,
                                                            &name,
                                                        )
                                                    {
                                                        this.object_notice = error;
                                                        cx.notify();
                                                        return false;
                                                    }
                                                    this.request_operation(
                                                        id,
                                                        OperationKind::CreateDatabase { name },
                                                        cx,
                                                    )
                                                })
                                                .unwrap_or(false);
                                            if sent {
                                                window.close_dialog(cx);
                                            }
                                        }),
                                )
                                .child(
                                    Button::new("cancel-create-database")
                                        .label("取消")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                ),
                        )
                        .child(
                            view.upgrade()
                                .map(|v| v.read(cx).object_notice.clone())
                                .unwrap_or_default(),
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
