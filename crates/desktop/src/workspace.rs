use crate::chat::{AiLog, ChatView};
use crate::database::{DatabaseView, OpenDatabase, OpenObjects};
use crate::panels::WorkbenchPanel;
use crate::settings::{SettingsRequest, SettingsView};
use crate::{
    catalog,
    locale::t,
    tabs::{CloseMode, CloseTabs, TabRouter},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IconName, Root, Selectable, StyledExt, Theme, ThemeMode, TitleBar,
    WindowExt,
    button::{Button, ButtonVariants},
    dock::{DockArea, DockEvent, DockItem, DockPlacement, PanelView, TabPanel},
    menu::{DropdownMenu, PopupMenuItem},
};
use std::sync::Arc;

const REGIONS: [DockPlacement; 3] = [
    DockPlacement::Left,
    DockPlacement::Bottom,
    DockPlacement::Right,
];

pub struct Workspace {
    notes: Entity<crate::note_library::NoteLibrary>,
    note_panels: std::collections::HashMap<String, Entity<crate::notebook::NoteView>>,
    _notes_subscription: Subscription,
    _notes_observe: Subscription,
    pending_note_close: Option<Vec<String>>,
    closing_notes_window: bool,

    dock: Entity<DockArea>,
    ai: Entity<ChatView>,
    settings: Entity<SettingsView>,
    center_tabs: Entity<TabPanel>,
    right_tabs: Entity<TabPanel>,
    settings_open: bool,
    settings_layout: Option<[bool; 3]>,
    closing_window: bool,
    panels: Vec<Arc<dyn PanelView>>,
    center_order: Vec<String>,
    right_order: Vec<String>,
    pending_close: Vec<String>,
    _tab_subscription: Subscription,
    _feedback_subscription: Subscription,
    update_status: String,
    checking_update: bool,
    update_task: Option<Task<()>>,
    _settings_subscription: Subscription,
    _status_observe: Subscription,
    navigation: Entity<WorkbenchPanel>,
    database: Entity<DatabaseView>,
    schema: Entity<WorkbenchPanel>,
    properties: Entity<WorkbenchPanel>,
    _detail_subscription: Subscription,
    _sql_draft_subscription: Subscription,
    _sql_settings_subscription: Subscription,
    _objects_subscription: Subscription,
    _table_subscription: Subscription,
    table_panels: std::collections::HashMap<String, Entity<crate::table_view::TableView>>,
    database_open: bool,
    _database_subscription: Subscription,
    _database_observe: Subscription,
    connections: bool,
    focus_snapshot: Option<([bool; 3], Option<FocusHandle>)>,
    _dock_subscription: Subscription,
}

fn icon_button(id: &'static str, icon: impl Into<Icon>, tooltip: &'static str, cx: &App) -> Button {
    Button::new(id)
        .ghost()
        .icon(icon)
        .tooltip(t(cx, tooltip))
        .w(px(32.))
        .h(px(32.))
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = cx.global::<crate::feedback::Hub>().0.clone();
        let feedback_subscription = cx.subscribe_in(
            &events,
            window,
            |_, _, event: &crate::feedback::Message, window, cx| {
                let full = crate::feedback::message_preview(&event.text);
                let copy = event.text.clone();
                window.push_notification(
                    gpui_component::notification::Notification::new()
                        .with_type(event.kind)
                        .id1::<crate::feedback::Events>(event.text.clone())
                        .autohide(!matches!(
                            event.kind,
                            gpui_component::notification::NotificationType::Error
                        ))
                        .content(move |_, _, cx| {
                            div()
                                .id("notification-details")
                                .max_h(px(180.))
                                .w_full()
                                .min_w_0()
                                .overflow_y_scroll()
                                .whitespace_normal()
                                .text_color(cx.theme().foreground)
                                .child(full.clone())
                                .into_any_element()
                        })
                        .when(
                            matches!(
                                event.kind,
                                gpui_component::notification::NotificationType::Error
                            ),
                            |notification| {
                                notification.action(move |_, _, cx| {
                                    let copy = copy.clone();
                                    Button::new("copy-error")
                                        .ghost()
                                        .icon(IconName::Copy)
                                        .tooltip(t(cx, "grid.error.copy"))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.to_string(),
                                            ));
                                            crate::feedback::success(t(cx, "feedback.copied"), cx);
                                        })
                                })
                            },
                        ),
                    cx,
                );
            },
        );
        let dock = cx.new(|cx| DockArea::new("workbench", Some(2), window, cx));
        let weak = dock.downgrade();
        let database = cx.new(|cx| DatabaseView::new(window, cx));
        let notes =
            cx.new(|cx| crate::note_library::NoteLibrary::new(database.clone(), window, cx));
        let notebook = cx.new(|cx| WorkbenchPanel::notebook(notes.clone(), cx));
        let navigation = cx.new(|cx| {
            WorkbenchPanel::info(
                "navigation",
                "navigation.projects",
                "notebook.welcome.title",
                "projects.empty.description",
                cx,
            )
        });
        navigation.update(cx, |panel, cx| panel.show_notes(notes.clone(), cx));
        let output = cx.new(AiLog::new);
        database.update(cx, |database, _| database.output = Some(output.clone()));
        let ai = cx.new(|cx| ChatView::new(output.clone(), window, cx));
        ai.update(cx, |chat, _| chat.databases = Some(database.downgrade()));
        let settings =
            cx.new(|cx| SettingsView::new(ai.clone(), database.read(cx).drivers(), window, cx));
        let properties = cx.new(|cx| {
            WorkbenchPanel::info(
                "properties",
                "panel.inspector.title",
                "notebook.welcome.title",
                "panel.inspector.description",
                cx,
            )
        });
        let schema = cx.new(|cx| WorkbenchPanel::database_objects(database.clone(), cx));
        let panels: Vec<Arc<dyn PanelView>> = vec![
            Arc::new(notebook.clone()),
            Arc::new(settings.clone()),
            Arc::new(database.clone()),
            Arc::new(ai.clone()),
            Arc::new(properties.clone()),
            Arc::new(schema.clone()),
        ];
        let center = DockItem::tab(notebook, &weak, window, cx);
        let DockItem::Tabs {
            view: center_tabs, ..
        } = &center
        else {
            unreachable!()
        };
        let center_tabs = center_tabs.clone();
        let left = DockItem::tab(navigation.clone(), &weak, window, cx);
        let right = DockItem::tabs(
            vec![
                Arc::new(ai.clone()),
                Arc::new(properties.clone()),
                Arc::new(schema.clone()),
            ],
            &weak,
            window,
            cx,
        );
        let bottom = DockItem::tab(output, &weak, window, cx);
        let DockItem::Tabs {
            view: right_tabs, ..
        } = &right
        else {
            unreachable!()
        };
        let right_tabs = right_tabs.clone();
        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_left_dock(left, Some(px(280.)), true, window, cx);
            dock.set_right_dock(
                right,
                Some(px(
                    (f32::from(window.viewport_size().width) * 0.38).clamp(360., 640.)
                )),
                true,
                window,
                cx,
            );
            dock.set_bottom_dock(bottom, Some(px(160.)), false, window, cx);
            dock.set_locked(true, window, cx);
            dock.set_toggle_button_visible(false, cx);
        });
        let subscription = cx.subscribe(&dock, |_, _, event, cx| {
            if matches!(event, DockEvent::LayoutChanged) {
                cx.notify();
            }
        });
        let settings_subscription = cx.subscribe_in(
            &ai,
            window,
            |this, _, event: &SettingsRequest, window, cx| match event {
                SettingsRequest::ToggleAiZoom => {
                    this.leave_focus(window, cx);
                    let zoomed = !this.ai.read(cx).is_zoomed();
                    this.dock.update(cx, |dock, cx| {
                        if zoomed {
                            dock.set_zoomed_in(this.ai.clone(), window, cx);
                        } else {
                            dock.set_zoomed_out(window, cx);
                        }
                    });
                    this.ai.update(cx, |chat, cx| chat.sync_zoom(zoomed, cx));
                }
                SettingsRequest::Open(category) => this.open_settings(*category, window, cx),
                SettingsRequest::CancelClose => {
                    this.closing_window = false;
                    this.pending_close.clear();
                }
                SettingsRequest::Close => {
                    if this.closing_window {
                        this.closing_window = false;
                        cx.defer_in(window, |this, window, cx| {
                            if this.prepare_close(window, cx) {
                                window.remove_window();
                            }
                        });
                        return;
                    }
                    let mut names = std::mem::take(&mut this.pending_close);
                    if !names.contains(&"settings".into()) {
                        names.push("settings".into());
                    }
                    this.close_names(&names, window, cx);
                }
                SettingsRequest::ReturnToChat => {
                    this.leave_focus(window, cx);
                    this.dock
                        .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
                    this.ai.update(cx, |chat, cx| chat.sync_zoom(false, cx));
                    if this
                        .right_tabs
                        .read(cx)
                        .active_panel(cx)
                        .is_none_or(|panel| panel.view().entity_id() != this.ai.entity_id())
                    {
                        // Rebuild only the tab container: the three tools and their
                        // editor/session entities remain alive, in the same order.
                        this.rebuild_tabs(false, window, cx);
                    }
                    if !this.dock.read(cx).is_dock_open(DockPlacement::Right, cx) {
                        this.dock.update(cx, |dock, cx| {
                            dock.toggle_dock(DockPlacement::Right, window, cx)
                        });
                    }
                    window.focus(&this.ai.read(cx).focus_handle(cx));
                }
            },
        );
        let db_subscription = cx.subscribe_in(
            &database,
            window,
            |this, _, _: &OpenDatabase, window, cx| {
                this.leave_focus(window, cx);
                this.dock
                    .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
                this.ai.update(cx, |ai, cx| ai.sync_zoom(false, cx));
                let active = this
                    .center_tabs
                    .read(cx)
                    .active_panel(cx)
                    .is_some_and(|p| p.view().entity_id() == this.database.entity_id());
                if !active {
                    this.center_tabs.update(cx, |tabs, cx| {
                        let panel = Arc::new(this.database.clone());
                        if this.database_open {
                            tabs.remove_panel(panel.clone(), window, cx);
                        }
                        tabs.add_panel(panel, window, cx);
                    });
                }
                if !active {
                    this.center_order.retain(|name| *name != "database-manager");
                    this.center_order.push("database-manager".into());
                }
                this.database_open = true;
                cx.notify();
            },
        );
        let db_observe = cx.observe(&database, |this, _, cx| {
            this.navigation.update(cx, |_, cx| cx.notify());
            this.schema.update(cx, |_, cx| cx.notify());
            cx.notify();
        });
        let objects_subscription =
            cx.subscribe_in(&database, window, |this, _, _: &OpenObjects, window, cx| {
                this.leave_focus(window, cx);
                this.dock
                    .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
                this.ai.update(cx, |ai, cx| ai.sync_zoom(false, cx));
                this.schema.update(cx, |panel, cx| panel.clear_detail(cx));
                this.right_tabs.update(cx, |tabs, cx| {
                    let panel = Arc::new(this.schema.clone());
                    tabs.remove_panel(panel.clone(), window, cx);
                    tabs.add_panel(panel, window, cx);
                });
                if !this.dock.read(cx).is_dock_open(DockPlacement::Right, cx) {
                    this.dock.update(cx, |dock, cx| {
                        dock.toggle_dock(DockPlacement::Right, window, cx)
                    });
                }
                cx.notify();
            });
        let sql_settings_subscription = cx.subscribe_in(
            &events,
            window,
            |this, _, _: &crate::feedback::OpenSqlSettings, window, cx| {
                this.open_settings(Some(crate::settings::Category::Database), window, cx);
                this.settings
                    .update(cx, |settings, cx| settings.select_sql(cx));
            },
        );
        let sql_draft_subscription = cx.subscribe_in(
            &events,
            window,
            |this, _, request: &crate::feedback::SqlDraft, window, cx| {
                this.leave_focus(window, cx);
                this.dock
                    .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
                this.ai.update(cx, |chat, cx| chat.sync_zoom(false, cx));
                this.right_tabs.update(cx, |tabs, cx| {
                    let panel: Arc<dyn PanelView> = Arc::new(this.ai.clone());
                    tabs.remove_panel(panel.clone(), window, cx);
                    tabs.add_panel(panel, window, cx);
                });
                if !this.dock.read(cx).is_dock_open(DockPlacement::Right, cx) {
                    this.dock.update(cx, |dock, cx| {
                        dock.toggle_dock(DockPlacement::Right, window, cx)
                    });
                }
                this.ai.update(cx, |chat, cx| {
                    chat.accept_sql_draft(request.clone(), window, cx)
                });
                cx.notify();
            },
        );
        cx.subscribe_in(
            &events,
            window,
            |this, _, event: &crate::feedback::DdlContext, window, cx| {
                this.database
                    .update(cx, |db, cx| db.set_ddl_context(event.clone(), cx));
                this.schema.update(cx, |panel, cx| panel.clear_detail(cx));
                if event.open {
                    this.leave_focus(window, cx);
                    this.right_tabs.update(cx, |tabs, cx| {
                        let panel: Arc<dyn PanelView> = Arc::new(this.schema.clone());
                        tabs.remove_panel(panel.clone(), window, cx);
                        tabs.add_panel(panel, window, cx);
                    });
                    if !this.dock.read(cx).is_dock_open(DockPlacement::Right, cx) {
                        this.dock.update(cx, |dock, cx| {
                            dock.toggle_dock(DockPlacement::Right, window, cx)
                        });
                    }
                }
            },
        )
        .detach();
        let detail_subscription = cx.subscribe_in(
            &events,
            window,
            |this, _, event: &crate::feedback::DetailRequest, window, cx| {
                let panel = if event.inspector {
                    this.properties.clone()
                } else {
                    this.schema.clone()
                };
                panel.update(cx, |panel, cx| panel.show_detail(event.clone(), window, cx));
                this.leave_focus(window, cx);
                this.right_tabs.update(cx, |tabs, cx| {
                    let panel: Arc<dyn PanelView> = Arc::new(panel);
                    tabs.remove_panel(panel.clone(), window, cx);
                    tabs.add_panel(panel, window, cx);
                });
                if !this.dock.read(cx).is_dock_open(DockPlacement::Right, cx) {
                    this.dock.update(cx, |dock, cx| {
                        dock.toggle_dock(DockPlacement::Right, window, cx)
                    });
                }
                cx.notify();
            },
        );
        let table_subscription = cx.subscribe_in(
            &database,
            window,
            |this, source, event: &crate::database::OpenTable, window, cx| {
                let key = format!(
                    "data:{}:{}:{}:{}:{:?}:{}",
                    event.connection,
                    event.database,
                    event.object.schema,
                    event.object.name,
                    event.mode,
                    event.generation
                );
                let panel = this
                    .table_panels
                    .entry(key.clone())
                    .or_insert_with(|| {
                        cx.new(|cx| {
                            crate::table_view::TableView::new(
                                key.clone(),
                                source.downgrade(),
                                event.clone(),
                                window,
                                cx,
                            )
                        })
                    })
                    .clone();
                this.leave_focus(window, cx);
                this.center_tabs.update(cx, |tabs, cx| {
                    let panel = Arc::new(panel);
                    tabs.remove_panel(panel.clone(), window, cx);
                    tabs.add_panel(panel, window, cx);
                });
                this.center_order.retain(|n| n != &key);
                this.center_order.push(key);
                cx.notify();
            },
        );
        let router = cx.global::<TabRouter>().0.clone();
        let tab_subscription =
            cx.subscribe_in(&router, window, |this, _, event: &CloseTabs, window, cx| {
                if crate::tabs::is_fixed(&event.name) {
                    return;
                }
                if event.name == "ai-output" {
                    if matches!(event.mode, CloseMode::Current) {
                        this.toggle_region(DockPlacement::Bottom, window, cx);
                    }
                    return;
                }
                let order = if this.center_order.contains(&event.name) {
                    &this.center_order
                } else {
                    &this.right_order
                };
                let names = crate::tabs::targets(order, &event.name, event.mode);
                if names.contains(&"settings".into()) {
                    if this.ai.read(cx).settings_saving() {
                        return;
                    }
                    this.pending_close = names;
                    this.open_settings(None, window, cx);
                    this.settings.update(cx, |settings, cx| settings.close(cx));
                } else {
                    this.close_names(&names, window, cx);
                }
            });
        let notes_subscription = cx.subscribe_in(
            &notes,
            window,
            |this, _, event: &crate::note_library::LibraryEvent, window, cx| match event {
                crate::note_library::LibraryEvent::Removed { key } => {
                    let key = key.clone();
                    cx.defer_in(window, move |this, window, cx| {
                        this.close_names(&[key], window, cx)
                    });
                }
                crate::note_library::LibraryEvent::Open { key, view } => {
                    let panel: Arc<dyn PanelView> = Arc::new(view.clone());
                    this.center_tabs.update(cx, |tabs, cx| {
                        tabs.remove_panel(panel.clone(), window, cx);
                        tabs.add_panel(panel, window, cx);
                    });
                    this.note_panels.insert(key.clone(), view.clone());
                    this.center_order.retain(|name| name != key);
                    this.center_order.push(key.clone());
                    cx.notify();
                }
                crate::note_library::LibraryEvent::Settled { key, success } => {
                    if !success {
                        if key.is_empty()
                            || this
                                .pending_note_close
                                .as_ref()
                                .is_some_and(|names| names.contains(key))
                        {
                            this.pending_note_close = None;
                        }
                        this.closing_notes_window = false;
                        return;
                    }
                    cx.defer_in(window, |this, window, cx| {
                        if let Some(names) = &this.pending_note_close
                            && !names
                                .iter()
                                .filter_map(|name| this.note_panels.get(name))
                                .any(|note| note.read(cx).pending())
                        {
                            let names = this.pending_note_close.take().unwrap();
                            this.close_names(&names, window, cx);
                        }
                        this.finish_window_after_notes(window, cx);
                    });
                }
            },
        );
        let notes_observe = cx.observe(&notes, |this, _, cx| {
            this.navigation.update(cx, |_, cx| cx.notify());
            cx.notify();
        });
        let status_observe = cx.observe(&ai, |_, _, cx| cx.notify());
        Self {
            notes,
            note_panels: Default::default(),
            _notes_subscription: notes_subscription,
            _notes_observe: notes_observe,
            pending_note_close: None,
            closing_notes_window: false,
            schema,
            properties,
            _detail_subscription: detail_subscription,
            _sql_draft_subscription: sql_draft_subscription,
            _sql_settings_subscription: sql_settings_subscription,
            table_panels: std::collections::HashMap::new(),
            _table_subscription: table_subscription,
            _objects_subscription: objects_subscription,
            database,
            database_open: false,
            _database_subscription: db_subscription,
            _database_observe: db_observe,
            panels,
            center_order: vec!["notebook".into()],
            right_order: vec!["ai-chat".into(), "properties".into(), "schema".into()],
            pending_close: vec![],
            _tab_subscription: tab_subscription,
            _feedback_subscription: feedback_subscription,
            update_status: cx
                .global::<crate::preferences::Preferences>()
                .load_error
                .clone()
                .unwrap_or_default(),
            checking_update: false,
            update_task: None,
            dock,
            ai,
            settings,
            center_tabs,
            right_tabs,
            settings_open: false,
            settings_layout: None,
            closing_window: false,
            _settings_subscription: settings_subscription,
            _status_observe: status_observe,
            navigation,
            connections: false,
            focus_snapshot: None,
            _dock_subscription: subscription,
        }
    }

    fn open_settings(
        &mut self,
        category: Option<crate::settings::Category>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        turbodbn_diagnostics::track(turbodbn_diagnostics::Event::SettingsOpened);
        self.leave_focus(window, cx);
        self.dock
            .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
        self.ai.update(cx, |chat, cx| chat.sync_zoom(false, cx));
        if !self.settings_open {
            self.settings_layout = Some(self.visibility(cx));
            self.restore_visibility([false; 3], window, cx);
        }
        self.settings
            .update(cx, |settings, cx| settings.select(category, cx));
        let panel = Arc::new(self.settings.clone());
        let active = self
            .center_tabs
            .read(cx)
            .active_panel(cx)
            .is_some_and(|p| p.view().entity_id() == self.settings.entity_id());
        if !active {
            self.center_tabs.update(cx, |tabs, cx| {
                if self.settings_open {
                    tabs.remove_panel(panel.clone(), window, cx);
                }
                tabs.add_panel(panel, window, cx);
            });
        }
        if !active {
            self.center_order.retain(|name| *name != "settings");
            self.center_order.push("settings".into());
        }
        self.settings_open = true;
        window.focus(&self.settings.read(cx).focus_handle(cx));
        cx.notify();
    }

    pub fn prepare_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let running = self.notes.read(cx).running(cx);
        if running || self.notes.read(cx).pending(cx) {
            if running
                || !cx
                    .global::<crate::preferences::Preferences>()
                    .general
                    .note_autosave
            {
                let view = cx.entity().downgrade();
                crate::feedback::confirm_labels(
                    t(cx, "notes.close_title"),
                    t(
                        cx,
                        if running {
                            "notes.close_running"
                        } else {
                            "notes.close_dirty"
                        },
                    ),
                    t(cx, "dbwork.draft.keep"),
                    t(cx, "notes.save_close"),
                    window,
                    cx,
                    move |window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.closing_notes_window = true;
                            this.notes.update(cx, |notes, cx| {
                                notes.stop(cx);
                                notes.flush(cx);
                            });
                            this.finish_window_after_notes(window, cx);
                        });
                    },
                );
            } else {
                self.closing_notes_window = true;
                self.notes.update(cx, |notes, cx| notes.flush(cx));
            }
            return false;
        }
        if self.settings.read(cx).pending(cx) {
            self.closing_window = true;
            self.open_settings(None, window, cx);
            self.settings.update(cx, |settings, cx| settings.close(cx));
            false
        } else {
            true
        }
    }

    fn flush_note_close(
        &mut self,
        names: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_note_close = Some(names.clone());
        for name in &names {
            if let Some(note) = self.note_panels.get(name) {
                note.update(cx, |note, cx| note.save_now(cx));
            }
        }
        if !names
            .iter()
            .filter_map(|name| self.note_panels.get(name))
            .any(|note| note.read(cx).pending())
        {
            self.pending_note_close = None;
            self.close_names(&names, window, cx);
        }
    }
    fn finish_window_after_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing_notes_window && !self.notes.read(cx).pending(cx) {
            self.closing_notes_window = false;
            if self.prepare_close(window, cx) {
                window.remove_window();
            }
        }
    }
    fn rebuild_tabs(&mut self, center: bool, window: &mut Window, cx: &mut Context<Self>) {
        let order = if center {
            &self.center_order
        } else {
            &self.right_order
        };
        let panels = order
            .iter()
            .filter_map(|name| {
                self.note_panels
                    .get(name)
                    .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                    .or_else(|| {
                        self.table_panels
                            .get(name)
                            .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                    })
                    .or_else(|| {
                        self.panels
                            .iter()
                            .find(|p| p.panel_name(cx) == name.as_str())
                            .cloned()
                    })
            })
            .collect();
        let item = DockItem::tabs(panels, &self.dock.downgrade(), window, cx);
        let DockItem::Tabs { view, .. } = &item else {
            unreachable!()
        };
        if center {
            self.center_tabs = view.clone();
        } else {
            self.right_tabs = view.clone();
        }
        // GPUI 0.5.1 exposes dock dimensions through its serializable layout.
        let right_size = serde_json::to_value(self.dock.read(cx).dump(cx))
            .ok()
            .and_then(|layout| layout.get("right_dock")?.get("size")?.as_f64())
            .map(|value| px(value as f32))
            .unwrap_or(px(480.));
        self.dock.update(cx, |dock, cx| {
            if center {
                dock.set_center(item, window, cx);
            } else {
                dock.set_right_dock(
                    item,
                    Some(right_size),
                    !self.right_order.is_empty(),
                    window,
                    cx,
                );
            }
        });
    }
    fn close_names(&mut self, names: &[String], window: &mut Window, cx: &mut Context<Self>) {
        let has_running = names
            .iter()
            .filter_map(|name| self.note_panels.get(name))
            .any(|note| note.read(cx).running(cx));
        let has_pending = names
            .iter()
            .filter_map(|name| self.note_panels.get(name))
            .any(|note| note.read(cx).pending());
        if has_running || has_pending {
            if has_running
                || !cx
                    .global::<crate::preferences::Preferences>()
                    .general
                    .note_autosave
            {
                let view = cx.entity().downgrade();
                let names = names.to_vec();
                crate::feedback::confirm_labels(
                    t(cx, "notes.close_title"),
                    t(
                        cx,
                        if has_running {
                            "notes.close_running"
                        } else {
                            "notes.close_dirty"
                        },
                    ),
                    t(cx, "dbwork.draft.keep"),
                    t(cx, "notes.save_close"),
                    window,
                    cx,
                    move |window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            for name in &names {
                                if let Some(note) = this.note_panels.get(name) {
                                    note.update(cx, |note, cx| note.stop(cx));
                                }
                            }
                            this.flush_note_close(names.clone(), window, cx);
                        });
                    },
                );
            } else {
                self.flush_note_close(names.to_vec(), window, cx);
            }
            return;
        }
        if names.iter().any(|name| name == "database-manager") {
            if self.database.read(cx).editor_busy() {
                return;
            }
            if self.database.read(cx).dirty(cx) {
                let view = cx.entity().downgrade();
                let names = names.to_vec();
                crate::feedback::confirm_labels(
                    t(cx, "dbwork.draft.close_title"),
                    t(cx, "dbwork.draft.close_body"),
                    crate::locale::t(cx, "dbwork.draft.keep"),
                    crate::locale::t(cx, "dbwork.draft.discard"),
                    window,
                    cx,
                    move |window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            if this.database.read(cx).editor_busy() {
                                return;
                            }
                            this.database
                                .update(cx, |database, cx| database.discard_editor(window, cx));
                            this.close_names(&names, window, cx);
                        });
                    },
                );
                return;
            }
        }
        self.leave_focus(window, cx);
        self.dock
            .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
        self.ai.update(cx, |chat, cx| chat.sync_zoom(false, cx));
        for name in names {
            if crate::tabs::is_fixed(name) {
                continue;
            }
            let Some(panel) = self
                .note_panels
                .get(name)
                .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                .or_else(|| {
                    self.table_panels
                        .get(name)
                        .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                })
                .or_else(|| {
                    self.panels
                        .iter()
                        .find(|p| p.panel_name(cx) == name.as_str())
                        .cloned()
                })
            else {
                continue;
            };
            if self.center_order.contains(name) {
                self.center_tabs
                    .update(cx, |tabs, cx| tabs.remove_panel(panel, window, cx));
            } else if self.right_order.contains(name) {
                self.right_tabs
                    .update(cx, |tabs, cx| tabs.remove_panel(panel, window, cx));
            }
        }
        for name in names {
            if self.note_panels.remove(name).is_some() {
                self.notes.update(cx, |notes, cx| notes.close(name, cx));
            }
        }
        self.center_order.retain(|n| !names.contains(n));
        self.right_order
            .retain(|n| crate::tabs::is_fixed(n) || !names.contains(n));
        self.settings_open = self.center_order.contains(&"settings".into());
        self.database_open = self.center_order.contains(&"database-manager".into());
        if names.contains(&"settings".into())
            && let Some(states) = self.settings_layout.take()
        {
            self.restore_visibility(states, window, cx);
        }
        if self.center_order.is_empty() {
            self.rebuild_tabs(true, window, cx);
        }
        if self.right_order.is_empty() {
            self.rebuild_tabs(false, window, cx);
        }
        cx.notify();
    }
    fn restore_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.center_order.contains(&"notebook".into()) {
            self.center_order.push("notebook".into());
            self.rebuild_tabs(true, window, cx);
        }
        self.right_order = vec!["ai-chat".into(), "properties".into(), "schema".into()];
        self.rebuild_tabs(false, window, cx);
        cx.notify();
    }
    fn check_update(&mut self, cx: &mut Context<Self>) {
        turbodbn_diagnostics::track(turbodbn_diagnostics::Event::UpdateChecked);
        if self.checking_update {
            return;
        }
        self.checking_update = true;
        self.update_status = t(cx, "common.status.loading").to_string();
        let work = cx
            .background_executor()
            .spawn(async { catalog::latest_release() });
        self.update_task = Some(cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |this, cx| {
                this.checking_update = false;
                this.update_status = match result {
                    Ok(catalog::ReleaseStatus::NoRelease) => t(cx, "updates.status.no_release"),
                    Ok(catalog::ReleaseStatus::Available(version)) => {
                        crate::locale::tr(cx, "updates.status.available", &[("version", &version)])
                    }
                    Ok(catalog::ReleaseStatus::Current(version)) => {
                        crate::locale::tr(cx, "updates.status.current", &[("version", &version)])
                    }
                    Err(error) => crate::locale::message(cx, &error),
                }
                .to_string();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn visibility(&self, cx: &App) -> [bool; 3] {
        REGIONS.map(|region| self.dock.read(cx).is_dock_open(region, cx))
    }

    fn restore_visibility(
        &mut self,
        states: [bool; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (region, open) in REGIONS.into_iter().zip(states) {
            if self.dock.read(cx).is_dock_open(region, cx) != open {
                self.dock
                    .update(cx, |dock, cx| dock.toggle_dock(region, window, cx));
            }
        }
        cx.notify();
    }

    fn leave_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((states, focus)) = self.focus_snapshot.take() {
            self.restore_visibility(states, window, cx);
            if let Some(focus) = focus {
                window.focus(&focus);
            }
        }
    }

    fn toggle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_snapshot.is_some() {
            self.leave_focus(window, cx);
        } else {
            self.focus_snapshot = Some((self.visibility(cx), window.focused(cx)));
            self.restore_visibility([false; 3], window, cx);
        }
    }

    fn toggle_region(
        &mut self,
        region: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_focus(window, cx);
        self.dock
            .update(cx, |dock, cx| dock.toggle_dock(region, window, cx));
        cx.notify();
    }

    fn navigate(&mut self, connections: bool, window: &mut Window, cx: &mut Context<Self>) {
        turbodbn_diagnostics::track(if connections {
            turbodbn_diagnostics::Event::ConnectionsOpened
        } else {
            turbodbn_diagnostics::Event::ProjectsOpened
        });
        self.leave_focus(window, cx);
        let open = self.visibility(cx)[0];
        let same = self.connections == connections;
        self.connections = connections;
        self.navigation.update(cx, |panel, cx| {
            if connections {
                panel.show_databases(self.database.clone(), cx);
            } else {
                panel.show_notes(self.notes.clone(), cx);
            }
        });
        if !open || same {
            self.dock.update(cx, |dock, cx| {
                dock.toggle_dock(DockPlacement::Left, window, cx)
            });
        }
        cx.notify();
    }

    fn help(window: &mut Window, cx: &mut App) {
        window.open_dialog(cx, |dialog, _, cx| {
            dialog.title(t(cx, "help.usage.title")).child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(crate::locale::t(cx, "help.navigation.description"))
                    .child(crate::locale::t(cx, "help.layout.description"))
                    .child(crate::locale::t(cx, "help.focus.description"))
                    .child(crate::locale::t(cx, "help.theme.description"))
                    .child(crate::locale::t(cx, "help.memory.description"))
                    .child(crate::feedback::actions().child(crate::feedback::close_button(cx))),
            )
        });
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialogs = Root::render_dialog_layer(window, cx);
        let notifications = Root::render_notification_layer(window, cx);
        let [left, bottom, right] = self.visibility(cx);
        let focused = self.focus_snapshot.is_some();
        let dark = cx.theme().mode.is_dark();
        let rail = div()
            .v_flex()
            .items_center()
            .w(px(48.))
            .h_full()
            .flex_shrink_0()
            .py_2()
            .gap_2()
            .bg(cx.theme().muted)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                icon_button("project", IconName::BookOpen, "notes.title", cx)
                    .selected(left && !self.connections)
                    .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx))),
            )
            .child(
                icon_button(
                    "connections",
                    Icon::default().path("icons/database.svg"),
                    "database.navigation.title",
                    cx,
                )
                .selected(left && self.connections)
                .on_click(cx.listener(|this, _, window, cx| this.navigate(true, window, cx))),
            )
            .child(div().flex_1())
            .child(
                icon_button("settings", IconName::Settings, "settings.title", cx).on_click(
                    cx.listener(|this, _, window, cx| this.open_settings(None, window, cx)),
                ),
            )
            .child(
                icon_button("help", IconName::Info, "help.title", cx).dropdown_menu({
                    let entity = cx.entity();
                    move |menu, _, cx| {
                        let update = entity.clone();
                        let restore = entity.clone();
                        menu.item(
                            PopupMenuItem::new(t(cx, "about.title"))
                                .icon(IconName::Info)
                                .on_click(|_, window, cx| {
                                    crate::about::open(window, cx);
                                }),
                        )
                        .item(
                            PopupMenuItem::new("GitHub")
                                .icon(IconName::GitHub)
                                .on_click(|_, _, cx| cx.open_url(catalog::REPOSITORY)),
                        )
                        .item(
                            PopupMenuItem::new(t(cx, "help.action.check_updates"))
                                .icon(crate::assets::refresh_icon())
                                .on_click(move |_, _, cx| {
                                    update.update(cx, |this, cx| this.check_update(cx))
                                }),
                        )
                        .item(
                            PopupMenuItem::new(t(cx, "help.action.releases"))
                                .icon(IconName::ExternalLink)
                                .on_click(|_, _, cx| cx.open_url(catalog::RELEASES)),
                        )
                        .item(
                            PopupMenuItem::new(t(cx, "workspace.action.restore_panels"))
                                .icon(IconName::PanelRight)
                                .on_click(move |_, window, cx| {
                                    restore.update(cx, |this, cx| this.restore_tabs(window, cx))
                                }),
                        )
                        .item(
                            PopupMenuItem::new(t(cx, "help.usage.title"))
                                .icon(IconName::BookOpen)
                                .on_click(|_, window, cx| Self::help(window, cx)),
                        )
                    }
                }),
            );
        let header = div()
            .h_flex()
            .h_full()
            .w_full()
            .flex_shrink_0()
            .px_3()
            .gap_1()
            .child(
                Icon::default()
                    .path("icons/data-note.svg")
                    .text_color(if dark { rgb(0x67e8d0) } else { rgb(0x087f78) }),
            )
            .child(
                div()
                    .flex_1()
                    .pl_2()
                    .text_size(rems(crate::typography::BODY))
                    .child("TurboDBNote"),
            )
            .child(
                icon_button("left", IconName::PanelLeft, "workspace.sidebar.toggle", cx)
                    .selected(left)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_region(DockPlacement::Left, window, cx)
                    })),
            )
            .child(
                icon_button(
                    "bottom",
                    IconName::PanelBottom,
                    "workspace.output.toggle",
                    cx,
                )
                .selected(bottom)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_region(DockPlacement::Bottom, window, cx)
                })),
            )
            .child(
                icon_button("right", IconName::PanelRight, "workspace.panels.toggle", cx)
                    .selected(right)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_region(DockPlacement::Right, window, cx)
                    })),
            )
            .child(div().w(px(1.)).h(px(16.)).mx_2().bg(cx.theme().border))
            .child(
                icon_button(
                    "focus",
                    if focused {
                        IconName::Minimize
                    } else {
                        IconName::Maximize
                    },
                    if focused {
                        "workspace.focus.exit"
                    } else {
                        "workspace.focus.enter"
                    },
                    cx,
                )
                .selected(focused)
                .on_click(cx.listener(|this, _, window, cx| this.toggle_focus(window, cx))),
            )
            .child(
                icon_button(
                    "theme",
                    if dark { IconName::Sun } else { IconName::Moon },
                    if dark {
                        "settings.theme.switch_light"
                    } else {
                        "settings.theme.switch_dark"
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.reset_theme(cx));
                    let mode = if cx.theme().mode.is_dark() {
                        ThemeMode::Light
                    } else {
                        ThemeMode::Dark
                    };
                    Theme::change(mode, Some(window), cx);
                    crate::locale::apply_font(cx);
                })),
            );
        div()
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().h(px(40.)).pl_0().child(header))
            .child(
                div().h_flex().flex_1().min_h_0().child(rail).child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(div().flex_1().min_h_0().child(self.dock.clone()))
                        .when(!focused, |el| {
                            el.child(
                                div()
                                    .h_flex()
                                    .gap_3()
                                    .px_3()
                                    .py_1()
                                    .flex_shrink_0()
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .text_size(rems(crate::typography::META))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .child(self.database.read(cx).summary(cx)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(self.update_status.clone()),
                                    )
                                    .child(div().flex_shrink_0().child(crate::locale::tr(
                                        cx,
                                        "status.ai.summary",
                                        &[(
                                            "state",
                                            t(cx, self.ai.read(cx).connection_status()).as_ref(),
                                        )],
                                    ))),
                            )
                        }),
                ),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.shift
                {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "j" => this.toggle_region(DockPlacement::Bottom, window, cx),
                    "b" => this.toggle_region(DockPlacement::Left, window, cx),
                    "," => this.open_settings(Some(crate::settings::Category::General), window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .children(dialogs)
            .child(
                div()
                    .absolute()
                    .top(px(40.))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .children(notifications),
            )
    }
}
