#[path = "designer_fields.rs"]
mod fields;
use crate::{
    database::{DatabaseView, OpenTable},
    feedback,
    locale::t,
    query_grid::QueryGrid,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    table::TableState,
};
use std::collections::HashMap;
use turbodbn_services::{
    DatabaseKind,
    database::workbench::{self, Action, Reply, Rows, Schema},
};

#[derive(Default)]
struct Models(HashMap<String, WeakEntity<Model>>);
impl Global for Models {}
struct Model {
    source: WeakEntity<DatabaseView>,
    target: OpenTable,
    schema: Option<Schema>,
    busy: bool,
    error: String,
    revision: u64,
}
impl Model {
    fn request(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let target = self.target.clone();
        let rx = source.update(cx, |source, cx| {
            source.mcp_request(
                target.connection,
                target.generation,
                target.database,
                action,
                cx,
            )
        });
        self.busy = true;
        self.error.clear();
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Reply::Schema(schema)) => {
                        if schema.columns.is_empty() && schema.ddl.trim().is_empty() {
                            this.schema = None;
                            this.error = "ddl.note.definition_missing".into();
                            cx.notify();
                            return;
                        }
                        if let Some(source) = this.source.upgrade() {
                            source.update(cx, |source, cx| {
                                source.remember_designer_schema(&this.target, &schema, cx)
                            });
                        }
                        this.schema = Some(schema);
                        this.revision += 1;
                    }
                    Ok(Reply::Executed) => {
                        feedback::success(t(cx, "designer.saved"), cx);
                        this.schema = None;
                        this.revision += 1;
                        this.request(Action::Describe(this.target.object.clone()), cx);
                    }
                    Err(error) => {
                        this.error = error;
                    }
                    _ => {
                        this.error = "mcp.failed".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
pub struct Designer {
    model: Entity<Model>,
    grid: Option<Entity<TableState<QueryGrid>>>,
    revision: u64,
    grid_observer: Option<Subscription>,
    text_mode: bool,
    scroll: ScrollHandle,
    _observe: Subscription,
}
impl Designer {
    pub fn new(
        source: WeakEntity<DatabaseView>,
        target: OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if cx.try_global::<Models>().is_none() {
            cx.set_global(Models::default());
        }
        let key = serde_json::to_string(&(
            target.connection,
            target.generation,
            &target.database,
            &target.object.schema,
            &target.object.name,
        ))
        .expect("design identity");
        let existing = cx.global::<Models>().0.get(&key).and_then(|m| m.upgrade());
        let model = existing.unwrap_or_else(|| {
            let model = cx.new(|_| Model {
                source,
                target,
                schema: None,
                busy: false,
                error: String::new(),
                revision: 0,
            });
            cx.global_mut::<Models>()
                .0
                .retain(|_, model| model.upgrade().is_some());
            cx.global_mut::<Models>().0.insert(key, model.downgrade());
            model
        });
        let observer = cx.observe(&model, |_, _, cx| cx.notify());
        cx.defer_in(window, |this, _, cx| {
            if this.model.read(cx).schema.is_none() && !this.model.read(cx).busy {
                this.model.update(cx, |m, cx| {
                    m.request(Action::Describe(m.target.object.clone()), cx)
                });
            }
        });
        Self {
            model,
            grid: None,
            grid_observer: None,
            revision: u64::MAX,
            text_mode: false,
            scroll: ScrollHandle::new(),
            _observe: observer,
        }
    }
    fn preview(&self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let m = model.read(cx);
        let Some(schema) = &m.schema else {
            return;
        };
        let ddl = schema.ddl.clone();
        let scope = m.target.clone();
        let label = m
            .source
            .upgrade()
            .and_then(|s| {
                s.read(cx).note_profile(scope.connection).map(|p| {
                    format!(
                        "{} · {} / {} · {}",
                        p.name,
                        p.kind.label(),
                        scope.database,
                        p.environment
                    )
                })
            })
            .unwrap_or_default();
        feedback::sql_preview(sql, label, window, cx, move |sql, _, cx| {
            let current = model.read(cx);
            if current.busy || current.schema.as_ref().is_none_or(|s| s.ddl != ddl) {
                feedback::error(t(cx, "dbwork.context.changed"), cx);
                return false;
            }
            let Some(source) = current.source.upgrade() else {
                return false;
            };
            if source.read(cx).note_generation(scope.connection) != scope.generation
                || source
                    .read(cx)
                    .note_profile(scope.connection)
                    .is_none_or(|p| !p.supports_mutation())
            {
                feedback::error(t(cx, "dbwork.context.changed"), cx);
                return false;
            }
            // Only DDL may leave this editor. The server checks the frozen original schema again.
            if !turbodbn_services::database::table_design::valid_target(
                scope.kind,
                &scope.database,
                &scope.object,
                &sql,
            ) {
                feedback::error(t(cx, "designer.ddl_only"), cx);
                return false;
            }

            if !cx
                .global::<crate::preferences::Preferences>()
                .database
                .command_enabled("alter")
            {
                feedback::error(t(cx, "permissions.global_alter"), cx);
                return false;
            }
            model.update(cx, |m, cx| {
                m.request(
                    Action::Execute {
                        sql,
                        guard: Some((scope.object.clone(), ddl.clone())),
                    },
                    cx,
                )
            });
            true
        });
    }
    fn edit(&self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).target.kind == DatabaseKind::MySql {
            self.edit_mysql(row, window, cx);
            return;
        }

        let m = self.model.read(cx);
        let Some(schema) = &m.schema else {
            return;
        };
        let old = row.and_then(|row| schema.columns.get(row)).cloned();
        let kind = m.target.kind;
        let target = workbench::qualified(kind, &m.target.database, &m.target.object);
        let schema = schema.clone();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(old.as_ref().map(|c| c.name.clone()).unwrap_or_default())
        });
        let definition = old
            .as_ref()
            .and_then(|c| {
                (kind == DatabaseKind::MySql)
                    .then(|| column_definition(kind, &schema.ddl, &c.name))
                    .flatten()
            })
            .unwrap_or_else(|| {
                old.as_ref()
                    .map(|c| c.data_type.clone())
                    .unwrap_or_else(|| {
                        turbodbn_services::database::table_design::default_type(kind).into()
                    })
            });
        let definition = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(definition)
        });
        let types = cx.new(|cx| fields::TypeChoices::new(&definition, kind, true, window, cx));
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let name = name.clone(); let definition = definition.clone(); let owner = owner.clone(); let old = old.clone(); let target = target.clone();
            dialog.title(t(cx,if old.is_some(){"designer.edit"}else{"designer.add"})).w(px(620.)).overlay_closable(false)
                .child(div().v_flex().gap_3().child(t(cx,"ddl.column.name")).child(Input::new(&name).cleanable(true))
                    .child(t(cx,"designer.definition")).child(div().h_flex().w_full().items_start().gap_2().child(div().flex_1().min_w_0().child(feedback::editable_multiline(&definition,false,cx).w_full().h(px(130.)))).child(div().w(px(180.)).flex_shrink_0().child(types.clone())))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t(cx,"designer.definition_hint")))
                    .child(feedback::actions().child(feedback::close_button(cx)).child(Button::new("designer-preview").icon(IconName::Check).label(t(cx,"dbwork.column.preview")).primary().on_click(move |_,window,cx| {
                        let name = name.read(cx).value().trim().to_string(); let definition = definition.read(cx).value().trim().to_string();
                        if name.is_empty() || name.len()>256 || name.contains('\0') || !turbodbn_services::database::table_design::valid_definition(kind,&definition) { feedback::error(t(cx,"designer.invalid"),cx); return; }
                        let q = workbench::quote(kind,&name);
                        let sql = match &old {
                            None => format!("ALTER TABLE {target} ADD COLUMN {q} {definition};"),
                            Some(old) if kind==DatabaseKind::MySql => format!("ALTER TABLE {target} CHANGE COLUMN {} {q} {definition};",workbench::quote(kind,&old.name)),
                            Some(old) if name != old.name && definition==old.data_type => format!("ALTER TABLE {target} RENAME COLUMN {} TO {q};",workbench::quote(kind,&old.name)),
                            Some(old) if kind==DatabaseKind::PostgreSql && name==old.name => format!("ALTER TABLE {target} ALTER COLUMN {q} TYPE {definition};"),
                            Some(old) if kind==DatabaseKind::ClickHouse && name==old.name => format!("ALTER TABLE {target} MODIFY COLUMN {q} {definition};"),
                            _ => { feedback::error(t(cx,"designer.edit_capability"),cx); return; }
                        };
                        window.close_dialog(cx);
                        let _=owner.update(cx,|this,cx| this.preview(sql.clone(),window,cx));
                    }))))
        });
    }
}
fn column_definition(kind: DatabaseKind, ddl: &str, name: &str) -> Option<String> {
    let prefix = format!("{} ", workbench::quote(kind, name));
    ddl.lines().map(str::trim).find_map(|line| {
        line.strip_prefix(&prefix)
            .map(|s| s.trim_end_matches(',').to_owned())
    })
}
impl Render for Designer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        let busy = m.busy;
        let schema = m.schema.clone();
        let target = m.target.clone();
        let error = m.error.clone();
        let writable = target.writable && target.object.kind == "table";
        if self.revision != m.revision {
            self.revision = m.revision;
            self.grid = None;
            self.grid_observer = None;
        }
        if self.grid.is_none()
            && let Some(schema) = &schema
        {
            let flag = |v: Option<bool>| {
                Some(
                    match v {
                        Some(true) => "✓",
                        Some(false) => "",
                        None => "—",
                    }
                    .to_string(),
                )
            };
            let rows = Rows {
                columns: [
                    "ddl.column.name",
                    "grid.structure.type",
                    "ddl.column.not_null",
                    "ddl.column.auto_increment",
                    "ddl.column.primary",
                    "ddl.column.extra",
                    "ddl.column.comment",
                ]
                .iter()
                .map(|k| t(cx, k).to_string())
                .collect(),
                values: schema
                    .columns
                    .iter()
                    .map(|c| {
                        vec![
                            Some(c.name.clone()),
                            Some(c.data_type.clone()),
                            flag(c.not_null),
                            flag(c.auto_increment),
                            flag(Some(c.primary)),
                            Some(c.extra.clone()),
                            Some(c.comment.clone()),
                        ]
                    })
                    .collect(),
            };
            self.grid = Some(cx.new(|cx| {
                TableState::new(
                    QueryGrid::new(rows, 0)
                        .with_column_widths(&[160., 150., 64., 64., 64., 220., 280.])
                        .without_row_numbers(),
                    window,
                    cx,
                )
            }));
        }
        if self.grid_observer.is_none()
            && let Some(grid) = &self.grid
        {
            self.grid_observer = Some(cx.observe(grid, |_, _, cx| cx.notify()));
        }
        let row = self
            .grid
            .as_ref()
            .and_then(|g| g.read(cx).delegate().selected)
            .map(|(r, _)| r);
        let mut body = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .gap_2()
            .p_2()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(format!("{} / {}", target.database, target.object.name)),
                    )
                    .child(
                        Button::new("design-refresh")
                            .ghost()
                            .icon(crate::assets::refresh_icon())
                            .tooltip(t(cx, "dbwork.refresh"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |m, cx| {
                                    m.request(Action::Describe(m.target.object.clone()), cx)
                                })
                            })),
                    )
                    .child(
                        Button::new("design-mode")
                            .ghost()
                            .label(if self.text_mode {
                                t(cx, "grid.structure.table").to_string()
                            } else {
                                "DDL".into()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.text_mode = !this.text_mode;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("design-copy")
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip(t(cx, "dbwork.copy.ddl"))
                            .disabled(schema.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(s) = &this.model.read(cx).schema {
                                    cx.write_to_clipboard(ClipboardItem::new_string(s.ddl.clone()));
                                    feedback::success(t(cx, "feedback.copied"), cx);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("design-add")
                            .ghost()
                            .icon(IconName::Plus)
                            .label(t(cx, "designer.add"))
                            .disabled(!writable || busy || schema.is_none())
                            .on_click(cx.listener(|this, _, w, cx| this.edit(None, w, cx))),
                    )
                    .child(
                        Button::new("design-edit")
                            .ghost()
                            .icon(IconName::Settings2)
                            .label(t(cx, "designer.edit"))
                            .disabled(!writable || busy || row.is_none())
                            .on_click(cx.listener(move |this, _, w, cx| this.edit(row, w, cx))),
                    )
                    .when(target.kind==DatabaseKind::MySql,|bar|bar.child(Button::new("design-primary").ghost().icon(IconName::Settings2).label(t(cx,"ddl.column.primary")).tooltip(t(cx,"designer.primary_hint")).disabled(!writable||busy||row.is_none()).on_click(cx.listener(move|this,_,window,cx|{
                        let m=this.model.read(cx);
                        if let Some(schema)=&m.schema && let Some(column)=row.and_then(|r|schema.columns.get(r)) {
                            match turbodbn_services::database::table_design::toggle_primary(&m.target.database,&m.target.object,schema,&column.name) {
                                Ok(sql)=>this.preview(sql,window,cx),Err(error)=>feedback::error(crate::locale::message(cx,&error),cx),
                            }
                        }
                    }))))
                    .child(
                        Button::new("design-drop")
                            .ghost()
                            .icon(IconName::Delete)
                            .label(t(cx, "designer.drop"))
                            .disabled(!writable || busy || row.is_none())
                            .on_click(cx.listener(move |this, _, w, cx| {
                                let m = this.model.read(cx);
                                if let Some(c) = row.and_then(|r| m.schema.as_ref()?.columns.get(r))
                                {
                                    let sql = format!(
                                        "ALTER TABLE {} DROP COLUMN {};",
                                        workbench::qualified(
                                            m.target.kind,
                                            &m.target.database,
                                            &m.target.object
                                        ),
                                        workbench::quote(m.target.kind, &c.name)
                                    );
                                    this.preview(sql, w, cx);
                                }
                            })),
                    ),
            );
        if busy {
            body = body.child(t(cx, "ddl.loading"));
        }
        if !error.is_empty() {
            body = body.child(
                div()
                    .whitespace_normal()
                    .child(crate::locale::message(cx, &error))
                    .child(
                        Button::new("design-retry")
                            .ghost()
                            .icon(crate::assets::refresh_icon())
                            .label(t(cx, "ddl.note.retry"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |m, cx| {
                                    m.request(Action::Describe(m.target.object.clone()), cx)
                                })
                            })),
                    ),
            );
        }
        if self.text_mode {
            if let Some(schema) = schema {
                let width = schema
                    .ddl
                    .lines()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0) as f32
                    * 8.5
                    + 40.;
                body = body.child(crate::content_scroll::view(
                    "design-ddl",
                    &self.scroll,
                    div().min_w_full().w(px(width)).child(
                        gpui_component::text::TextView::markdown(
                            "design-sql",
                            crate::cell::sql_reading(&schema.ddl, target.kind),
                            window,
                            cx,
                        )
                        .selectable(true),
                    ),
                ));
            }
        } else if let Some(grid) = &self.grid {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(crate::query_grid::view(grid, cx)),
            );
        }
        body
    }
}
