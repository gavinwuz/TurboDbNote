use super::*;
use gpui_component::WindowExt;
impl DatabaseView {
    pub(super) fn clear_object_view(&mut self) {
        self.note_columns.clear();
        self.object_scroll = UniformListScrollHandle::new();
        self.selected_object = None;
        self.object_items.clear();
        self.object_catalog_loaded = false;
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
            self.ddl.pending = false;
        }
        let names = self.catalog(&profile);
        let database = (!profile.database.is_empty())
            .then(|| profile.database.clone())
            .or_else(|| {
                self.navigation
                    .databases
                    .get(&id)
                    .filter(|db| names.contains(db))
                    .cloned()
            })
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
        let log_kind = match &kind {
            OperationKind::CreateDatabase { .. } => crate::operation_log::Kind::Create,
            OperationKind::CopyDatabase { .. } => crate::operation_log::Kind::Copy,
            _ => crate::operation_log::Kind::Query,
        };
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
        self.operation_log(id, log_kind, crate::operation_log::Phase::Started, None, cx);
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
        let columns_only = self.ddl.pending
            || matches!(&kind, OperationKind::Workbench(request) if matches!(request.action, Action::NoteColumns(_) | Action::Ddl(_) | Action::NotePrepare { .. } | Action::NoteExecute { .. }));
        self.active_operation = Some((id, request, kind));
        if !columns_only {
            self.object_notice = "正在读取 / 执行…".into();
        }
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
        let log_kind = match &kind {
            OperationKind::CreateDatabase { .. } => crate::operation_log::Kind::Create,
            OperationKind::CopyDatabase { .. } => crate::operation_log::Kind::Copy,
            _ => crate::operation_log::Kind::Query,
        };
        self.operation_log(
            id,
            log_kind,
            if result.is_ok() {
                crate::operation_log::Phase::Succeeded
            } else {
                crate::operation_log::Phase::Failed
            },
            result.as_ref().err().map(String::as_str),
            cx,
        );
        if result.is_ok() {
            self.remember_connection(id, cx);
        }
        if let Some(sender) = self.mcp_target.take() {
            let _ = sender.try_send(match result {
                Ok(OperationResult::Workbench(reply)) => Ok(*reply),
                _ => Err("mcp.failed".into()),
            });
            cx.notify();
            return;
        }
        if self.ddl.pending {
            self.ddl.pending = false;
            self.receive_ddl(result, cx);
            return;
        }
        if let OperationKind::Workbench(request) = &kind
            && let Action::NoteColumns(object) = &request.action
        {
            match result {
                Ok(OperationResult::Workbench(reply)) => {
                    if let Reply::Schema(schema) = *reply {
                        self.remember_note_columns(id, &request.database, object, &schema.columns);
                    }
                }
                Err(error) => feedback::error(error, cx),
                _ => {}
            }
            cx.emit(NoteColumnsReady(id));
            cx.notify();
            return;
        }

        if let Some(target) = self.workbench_target.take() {
            let result = result.and_then(|r| match r {
                OperationResult::Workbench(r) => Ok(*r),
                _ => Err("响应类型不匹配".into()),
            });
            let _ = target.update(cx, |view, cx| view.receive(result, cx));
            self.object_notice = format!(
                "{} 个对象",
                self.catalog_objects.len().max(self.object_items.len())
            );
            cx.notify();
            return;
        }
        match result {
            Ok(OperationResult::Workbench(reply)) => match *reply {
                Reply::Catalog { objects, notice } => {
                    self.catalog_objects = objects;
                    self.object_catalog_loaded = true;
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
                    self.note_columns
                        .retain(|(connection, _, _, _), _| *connection != id);
                }
                Reply::Page { .. }
                | Reply::GridPage { .. }
                | Reply::Count(_)
                | Reply::NotePrepared(_)
                | Reply::NoteResult(_)
                | Reply::RedisValue(_) => {}
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
                if matches!(kind, OperationKind::Objects { .. }) {
                    self.redis_pattern = "*".into();
                }
                if matches!(
                    kind,
                    OperationKind::Objects {
                        cursor: Some(_),
                        ..
                    }
                ) || matches!(&kind,OperationKind::Workbench(r) if matches!(r.action,Action::RedisScan {cursor:Some(_),..}))
                {
                    for item in items {
                        if !self.object_items.iter().any(|old| {
                            old.schema == item.schema
                                && old.name == item.name
                                && old.key == item.key
                        }) && self.object_items.len() < 5000
                        {
                            self.object_items.push(item);
                        }
                    }
                } else {
                    self.object_items = items;
                }
                self.object_catalog_loaded = true;
                self.object_cursor = cursor;
                self.object_notice =
                    notice.unwrap_or_else(|| format!("{} 个对象", self.object_items.len()));
            }
            Ok(OperationResult::Details(details)) => {
                self.detail_scroll = UniformListScrollHandle::new();
                self.object_details = details;
                self.object_notice = "对象详情已更新".into();
            }
            Ok(OperationResult::Copied {
                name,
                tables,
                include_data: _,
            }) => {
                self.object_notice = crate::locale::tr(
                    cx,
                    "dbwork.copy.done",
                    &[("name", &name), ("count", &tables.to_string())],
                )
                .to_string();
                feedback::success(self.object_notice.clone(), cx);
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
        self.database_creation_dialog(id, None, window, cx);
    }
    fn database_creation_dialog(
        &mut self,
        id: Uuid,
        source: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use std::{cell::Cell, rc::Rc};
        let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let copying = source.is_some();
        let sqlite = profile.kind == DatabaseKind::Sqlite;
        if copying && !matches!(profile.kind, DatabaseKind::MySql | DatabaseKind::Sqlite)
            || !copying && !profile.kind.can_create_database()
        {
            return;
        }
        let legacy40 = self
            .info
            .get(&id)
            .is_some_and(|info| info.version.starts_with("4.0"));
        let supports_mb4 = self
            .info
            .get(&id)
            .is_none_or(|info| mysql::modern_version(&info.version));
        let show_charset =
            matches!(profile.kind, DatabaseKind::MySql | DatabaseKind::PostgreSql) && !legacy40;
        let target = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(if sqlite {
                    String::new()
                } else {
                    source
                        .as_ref()
                        .map(|s| format!("{s}_copy"))
                        .unwrap_or_default()
                })
                .placeholder(crate::locale::t(
                    cx,
                    if sqlite {
                        "dbwork.copy.file"
                    } else {
                        "feedback.database.name"
                    },
                ))
        });
        let charset = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::locale::t(
                cx,
                if copying {
                    "dbwork.copy.charset"
                } else {
                    "dbwork.create.charset_default"
                },
            ))
        });
        let include_data = Rc::new(Cell::new(false));
        let view = cx.entity().downgrade();
        let generation = self.generation.get(&id).copied().unwrap_or_default();
        window.open_dialog(cx, move |dialog, _, cx| {
            let target_field = target.clone();
            let charset_field = charset.clone();
            let data = include_data.clone();
            let view = view.clone();
            let profile = profile.clone();
            let source = source.clone();
            let mut fields = div()
                .v_flex()
                .gap_2()
                .child(format!("{} · {}", profile.name, profile.environment))
                .when_some(source.clone(), |el, source| {
                    el.child(crate::locale::tr(
                        cx,
                        "dbwork.copy.source",
                        &[("source", &source)],
                    ))
                })
                .child(Input::new(&target));
            if sqlite {
                let field = target.clone();
                let directory = std::path::Path::new(&profile.sqlite_path)
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_path_buf();
                fields = fields.child(
                    Button::new("copy-database-file")
                        .icon(IconName::FolderOpen)
                        .label(crate::locale::t(cx, "dbwork.copy.choose_file"))
                        .on_click(move |_, window, cx| {
                            let chosen =
                                cx.prompt_for_new_path(&directory, Some("database-copy.db"));
                            let field = field.clone();
                            window
                                .spawn(cx, async move |cx| {
                                    if let Ok(Ok(Some(path))) = chosen.await {
                                        let _ = cx.update(|window, cx| {
                                            field.update(cx, |input, cx| {
                                                input.set_value(
                                                    path.display().to_string(),
                                                    window,
                                                    cx,
                                                )
                                            })
                                        });
                                    }
                                })
                                .detach();
                        }),
                );
            }
            if show_charset {
                let field = charset.clone();
                let pg = profile.kind == DatabaseKind::PostgreSql;
                fields = fields.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(Input::new(&charset).flex_1())
                        .child(
                            Button::new("database-charset")
                                .icon(IconName::ChevronDown)
                                .tooltip(crate::locale::t(cx, "dbwork.create.charset"))
                                .dropdown_menu(move |mut menu, _, cx| {
                                    let choices = if pg {
                                        vec!["", "UTF8", "LATIN1", "WIN1252"]
                                    } else if supports_mb4 {
                                        vec!["", "utf8mb4", "utf8", "gbk", "gb2312", "latin1"]
                                    } else {
                                        vec!["", "utf8", "gbk", "gb2312", "latin1"]
                                    };
                                    for value in choices {
                                        let field = field.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(if value.is_empty() {
                                                crate::locale::t(
                                                    cx,
                                                    if copying {
                                                        "dbwork.copy.charset"
                                                    } else {
                                                        "dbwork.create.charset_default"
                                                    },
                                                )
                                                .to_string()
                                            } else {
                                                value.to_string()
                                            })
                                            .on_click(move |_, window, cx| {
                                                field.update(cx, |input, cx| {
                                                    input.set_value(value, window, cx)
                                                })
                                            }),
                                        );
                                    }
                                    menu
                                }),
                        ),
                );
            } else if legacy40 {
                fields = fields.child(crate::locale::t(cx, "dbwork.create.legacy_charset"));
            }
            if copying {
                let data = include_data.clone();
                fields = fields
                    .child(
                        Switch::new("copy-database-data")
                            .label(crate::locale::t(cx, "dbwork.copy.include_data"))
                            .checked(include_data.get())
                            .on_click(move |checked, _, cx| {
                                data.set(*checked);
                                cx.refresh_windows();
                            }),
                    )
                    .child(crate::locale::t(
                        cx,
                        if sqlite {
                            "dbwork.copy.sqlite_scope"
                        } else {
                            "dbwork.copy.mysql_scope"
                        },
                    ));
            }
            dialog
                .title(crate::locale::t(
                    cx,
                    if copying {
                        "dbwork.database.copy"
                    } else {
                        "dbwork.database.new"
                    },
                ))
                .w(px(560.))
                .overlay_closable(false)
                .child(fields)
                .footer(move |_, _, _, cx| {
                    let target = target_field.clone();
                    let charset = charset_field.clone();
                    let data = data.clone();
                    let view = view.clone();
                    let profile = profile.clone();
                    let source = source.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("confirm-create-database")
                                    .primary()
                                    .icon(if copying {
                                        IconName::Copy
                                    } else {
                                        IconName::Plus
                                    })
                                    .label(crate::locale::t(
                                        cx,
                                        if copying {
                                            "dbwork.database.copy"
                                        } else {
                                            "feedback.database.create"
                                        },
                                    ))
                                    .on_click(move |_, window, cx| {
                                        let name = target.read(cx).value().trim().to_string();
                                        let charset = charset.read(cx).value().trim().to_string();
                                        let sql = if sqlite {
                                            if !std::path::Path::new(&name).is_absolute()
                                                || std::path::Path::new(&name).exists()
                                            {
                                                feedback::error(
                                                    crate::locale::t(cx, "dbwork.copy.new_file"),
                                                    cx,
                                                );
                                                return;
                                            }
                                            String::new()
                                        } else {
                                            match turbodbn_services::database::create_database_sql(
                                                profile.kind,
                                                &name,
                                                &charset,
                                            ) {
                                                Ok(sql) => sql,
                                                Err(error) => {
                                                    feedback::error(error, cx);
                                                    return;
                                                }
                                            }
                                        };
                                        let with_data = data.get();
                                        let action = if let Some(source) = &source {
                                            if source.eq_ignore_ascii_case(&name) {
                                                feedback::error(
                                                    crate::locale::t(cx, "dbwork.copy.different"),
                                                    cx,
                                                );
                                                return;
                                            }
                                            OperationKind::CopyDatabase {
                                                source: source.clone(),
                                                target: name.clone(),
                                                charset,
                                                include_data: with_data,
                                            }
                                        } else {
                                            OperationKind::CreateDatabase {
                                                name: name.clone(),
                                                charset,
                                            }
                                        };
                                        let view = view.clone();
                                        let display = if sqlite {
                                            std::path::Path::new(&name)
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned()
                                        } else {
                                            name
                                        };
                                        let mut message = format!(
                                            "{} · {} / {}",
                                            profile.name, profile.environment, display
                                        );
                                        if copying {
                                            message.push_str(&format!(
                                                "\n{}",
                                                crate::locale::t(
                                                    cx,
                                                    if with_data {
                                                        "dbwork.copy.confirm_data"
                                                    } else {
                                                        "dbwork.copy.confirm_schema"
                                                    }
                                                )
                                            ));
                                        } else {
                                            message.push_str(&format!("\n{sql}"));
                                        }
                                        feedback::confirm(
                                            crate::locale::t(cx, "feedback.database.confirm"),
                                            message,
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
                                                            action.clone(),
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
                    ]
                })
        });
    }
    fn scan_redis(&mut self, id: Uuid, database: String, more: bool, cx: &mut Context<Self>) {
        if !more {
            let text = self.object_filter.read(cx).value().trim().to_string();
            self.redis_pattern = if text.is_empty() { "*".into() } else { text };
            self.object_items.clear();
            self.object_cursor = None;
        }
        if self.object_items.len() >= 5000 {
            return;
        }
        self.request_operation(
            id,
            OperationKind::Workbench(Box::new(workbench::Request {
                database,
                action: Action::RedisScan {
                    cursor: if more {
                        self.object_cursor.clone()
                    } else {
                        None
                    },
                    pattern: self.redis_pattern.clone(),
                },
            })),
            cx,
        );
    }
    pub(super) fn render_redis_tree(
        &self,
        id: Uuid,
        database: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        use std::collections::BTreeSet;
        let connected = self.sessions.contains_key(&id);
        let busy = self.active_operation.is_some();
        let mut folders = BTreeSet::new();
        let mut rows = Vec::<(String, usize, Option<usize>, String)>::new();
        for (index, item) in self.object_items.iter().enumerate() {
            let valid = item
                .key
                .as_ref()
                .is_none_or(|key| std::str::from_utf8(key).is_ok());
            let parts = if valid {
                item.name.splitn(5, ':').collect::<Vec<_>>()
            } else {
                vec![item.name.as_str()]
            };
            let mut prefix = String::new();
            let mut visible = true;
            for (depth, part) in parts.iter().enumerate().take(parts.len().saturating_sub(1)) {
                if depth > 0 {
                    prefix.push(':');
                }
                prefix.push_str(part);
                if !visible {
                    break;
                }
                if folders.insert(prefix.clone()) {
                    rows.push((prefix.clone(), depth, None, (*part).into()));
                }
                visible = self.expanded_objects.contains(&format!("redis:{prefix}"));
            }
            if visible {
                rows.push((
                    item.name.clone(),
                    parts.len() - 1,
                    Some(index),
                    parts.last().unwrap_or(&"").to_string(),
                ));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.is_some().cmp(&b.2.is_some())));
        let scan_db = database.clone();
        let more_db = database.clone();
        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(Input::new(&self.object_filter).cleanable(true).flex_1())
                    .child(
                        Button::new("redis-filter")
                            .icon(IconName::Search)
                            .tooltip(crate::locale::t(cx, "redis.filter"))
                            .disabled(busy || !connected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.scan_redis(id, scan_db.clone(), false, cx)
                            })),
                    ),
            )
            .child(
                uniform_list(
                    "redis-key-tree",
                    rows.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                let (path, depth, item, label) = rows[index].clone();
                                let key = format!("redis:{path}");
                                let expanded = this.expanded_objects.contains(&key);
                                let db = database.clone();
                                Button::new(("redis-tree-row", index))
                                    .ghost()
                                    .h(px(28.))
                                    .w_full()
                                    .justify_start()
                                    .pl(px(8. + depth as f32 * 12.))
                                    .icon(if item.is_some() {
                                        IconName::File
                                    } else if expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .label(label)
                                    .tooltip(path)
                                    .disabled(busy || !connected)
                                    .on_click(cx.listener(
                                        move |this, event: &ClickEvent, _, cx| {
                                            if let Some(index) = item {
                                                if event.click_count() != 2 {
                                                    return;
                                                }
                                                let item = &this.object_items[index];
                                                let bytes = item
                                                    .key
                                                    .as_deref()
                                                    .unwrap_or(item.name.as_bytes());
                                                let hex = bytes
                                                    .iter()
                                                    .map(|b| format!("{b:02x}"))
                                                    .collect::<String>();
                                                cx.emit(OpenTable {
                                                    connection: id,
                                                    database: db.clone(),
                                                    object: Object {
                                                        schema: hex.clone(),
                                                        name: item.name.clone(),
                                                        kind: "redis-key".into(),
                                                        comment: String::new(),
                                                        parent: hex,
                                                    },
                                                    mode: crate::table_view::Mode::Grid,
                                                    generation: this.note_generation(id),
                                                    kind: DatabaseKind::Redis,
                                                    writable: false,
                                                });
                                            } else {
                                                if !this.expanded_objects.remove(&key) {
                                                    this.expanded_objects.insert(key.clone());
                                                }
                                                cx.notify();
                                            }
                                        },
                                    ))
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .min_h_0(),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .h_flex()
                    .gap_1()
                    .child(self.object_notice.clone())
                    .child(
                        Button::new("redis-scope-info")
                            .ghost()
                            .icon(IconName::Info)
                            .tooltip(crate::locale::t(cx, "redis.scope")),
                    ),
            )
            .child(
                Button::new("redis-scan-more")
                    .icon(crate::assets::refresh_icon())
                    .label(crate::locale::t(cx, "redis.scan_more"))
                    .disabled(
                        busy || !connected
                            || self.object_cursor.is_none()
                            || self.object_items.len() >= 5000,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.scan_redis(id, more_db.clone(), true, cx)
                    })),
            )
            .into_any_element()
    }
}

fn is_metadata_read(kind: &OperationKind) -> bool {
    match kind {
        OperationKind::Databases
        | OperationKind::Objects { .. }
        | OperationKind::Details { .. } => true,
        OperationKind::Workbench(request) => matches!(
            request.action,
            Action::Catalog
                | Action::Describe(_)
                | Action::Ddl(_)
                | Action::NoteColumns(_)
                | Action::RedisScan { .. }
        ),
        OperationKind::CreateDatabase { .. } | OperationKind::CopyDatabase { .. } => false,
    }
}
