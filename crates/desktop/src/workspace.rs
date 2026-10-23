#[path = "workspace_layout.rs"]
mod layout_ui;
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
    permission_panels: std::collections::HashMap<String, Entity<crate::database::Privileges>>,
    _permission_subscription: Subscription,
    pending_table_restore: Vec<layout_ui::SavedTab>,
    table_drafts: std::collections::HashMap<String, Subscription>,
    restored_center: Vec<Vec<String>>,
    center_sizes: Vec<f32>,
    layout_task: Option<Task<()>>,
    layout_ready: bool,
    layout_error: bool,
    center_groups: Vec<Vec<String>>,
    center_views: Vec<Entity<TabPanel>>,
    active_center: usize,
    _layout_subscription: Subscription,
    notes: Entity<crate::note_library::NoteLibrary>,
    note_panels: std::collections::HashMap<String, Entity<crate::notebook::NoteView>>,
    note_views: std::collections::HashMap<String, Entity<crate::notebook::NotePane>>,
    restored_note_views: Vec<(String, String)>,
    _notes_subscription: Subscription,
    _notes_observe: Subscription,
    pending_note_close: Option<Vec<String>>,
    closing_notes_window: bool,
    #[cfg(target_os = "windows")]
    tray: Option<crate::windows_tray::Tray>,
    #[cfg(target_os = "windows")]
    tray_ready: bool,
    #[cfg(target_os = "windows")]
    tray_epoch: u64,

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
    _new_note_subscription: Subscription,
    _feedback_subscription: Subscription,
    update_status: String,
    updates: Entity<crate::updates::UpdateView>,
    _update_subscription: Subscription,
    pending_install: Option<turbodbn_services::updates::Download>,
    install_launching: bool,
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
        cx.defer_in(window, |this, window, cx| {
            this.load_center_layout(window, cx)
        });
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
                        .id1::<crate::feedback::Events>(
                            event.id.clone().unwrap_or_else(|| event.text.clone()),
                        )
                        .autohide(
                            !event.persistent
                                && !matches!(
                                    event.kind,
                                    gpui_component::notification::NotificationType::Error
                                ),
                        )
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
                            ) && event.action.is_none(),
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
                        )
                        .when_some(event.action.clone(), |notification, action| {
                            notification.action(move |_, _, cx| {
                                let dismiss = cx.entity().downgrade();
                                let handler = action.handler.clone();
                                Button::new("notification-resolve")
                                    .ghost()
                                    .label(action.label.clone())
                                    .icon(IconName::ArrowRight)
                                    .on_click(move |_, window, cx| {
                                        let _ = dismiss.update(cx, |notification, cx| {
                                            notification.dismiss(window, cx)
                                        });
                                        handler(window, cx);
                                    })
                            })
                        }),
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
        database
            .read(cx)
            .drivers()
            .read(cx)
            .components
            .clone()
            .update(cx, |components, _| components.output = Some(output.clone()));
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
        let subscription = cx.subscribe(&dock, |this, source, event, cx| {
            if matches!(event, DockEvent::LayoutChanged) {
                this.observe_center_sizes(&source, cx);
                this.save_center_layout(cx);
                cx.notify();
            }
        });
        let settings_subscription = cx.subscribe_in(
            &ai,
            window,
            |this, _, event: &SettingsRequest, window, cx| match event {
                SettingsRequest::ToggleSidebarLayout => {
                    this.settings
                        .update(cx, |settings, cx| settings.toggle_sidebar_layout(cx));
                }
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
                    this.pending_install = None;
                    this.pending_close.clear();
                }
                SettingsRequest::Close => {
                    if this.closing_window {
                        this.closing_window = false;
                        cx.defer_in(window, |this, window, cx| {
                            if this.prepare_close(window, cx) {
                                this.finish_close(window, cx);
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
                this.put_center_panel(
                    "database-manager",
                    Arc::new(this.database.clone()),
                    window,
                    cx,
                );
                if !this
                    .center_order
                    .iter()
                    .any(|name| name == "database-manager")
                {
                    this.center_order.push("database-manager".into());
                }
                this.record_center_open("database-manager", cx);
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
        cx.subscribe_in(
            &database,
            window,
            |this, _, event: &SettingsRequest, window, cx| match event {
                SettingsRequest::Open(category) => {
                    this.open_settings(*category, window, cx);
                    this.settings
                        .update(cx, |settings, cx| settings.select_database_layout(cx));
                }
                SettingsRequest::ToggleSidebarLayout => {
                    this.settings
                        .update(cx, |settings, cx| settings.toggle_sidebar_layout(cx));
                }
                _ => {}
            },
        )
        .detach();
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
                if event.refresh {
                    cx.notify();
                    return;
                }
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
                if event.mode == crate::table_view::Mode::Note {
                    this.notes.update(cx, |notes, cx| {
                        notes.new_table_query(event.clone(), window, cx)
                    });
                    return;
                }

                let generated_key = serde_json::to_string(&(
                    "data",
                    event.connection,
                    &event.database,
                    &event.object.schema,
                    &event.object.name,
                    format!("{:?}", event.mode),
                    event.generation,
                ))
                .expect("tab identity");
                let key = this
                    .table_panels
                    .iter()
                    .find(|(_, panel)| panel.read(cx).matches_target(event))
                    .map(|(key, _)| key.clone())
                    .unwrap_or(generated_key);
                this.activate_center(&key, cx);
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
                this.watch_table(&key, &panel, cx);
                panel.update(cx, |panel, cx| panel.resume_restored(window, cx));
                this.leave_focus(window, cx);
                this.put_center_panel(&key, Arc::new(panel), window, cx);
                this.center_order.retain(|n| n != &key);
                this.center_order.push(key.clone());
                this.record_center_open(&key, cx);
                cx.notify();
            },
        );
        let permission_subscription = cx.subscribe_in(
            &database,
            window,
            |this, _, event: &crate::database::OpenPrivileges, window, cx| {
                let key = event.0.read(cx).key().to_owned();
                let panel = this
                    .permission_panels
                    .entry(key.clone())
                    .or_insert_with(|| event.0.clone())
                    .clone();
                this.activate_center(&key, cx);
                this.leave_focus(window, cx);
                this.put_center_panel(&key, Arc::new(panel), window, cx);
                if !this.center_order.contains(&key) {
                    this.center_order.push(key.clone());
                }
                this.record_center_open(&key, cx);
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
                if matches!(event.mode, CloseMode::Current)
                    && this.close_note_view(&event.name, window, cx)
                {
                    return;
                }
                let order = if this.center_order.contains(&event.name) {
                    this.center_groups
                        .iter()
                        .find(|group| group.contains(&event.name))
                        .unwrap_or(&this.center_order)
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
        let layout_subscription = cx.subscribe_in(
            &router,
            window,
            |this, _, event: &crate::tabs::LayoutAction, window, cx| {
                let event = event.clone();
                match event {
                    crate::tabs::LayoutAction::Activate(name) => this.activate_center(&name, cx),
                    event => cx.defer_in(window, move |this, window, cx| {
                        this.change_center(event, window, cx)
                    }),
                }
            },
        );
        let new_note_subscription = cx.subscribe_in(
            &cx.global::<TabRouter>().0.clone(),
            window,
            |this, _, event: &crate::tabs::NewNote, window, cx| {
                this.activate_center(&event.0, cx);
                this.notes
                    .update(cx, |notes, cx| notes.new_note(window, cx));
            },
        );
        let notes_subscription = cx.subscribe_in(
            &notes,
            window,
            |this, _, event: &crate::note_library::LibraryEvent, window, cx| match event {
                crate::note_library::LibraryEvent::Removed { key } => {
                    let key = key.clone();
                    cx.defer_in(window, move |this, window, cx| {
                        let mut names = vec![key.clone()];
                        names.extend(
                            this.note_views
                                .iter()
                                .filter(|(_, v)| v.read(cx).owner.read(cx).key == key)
                                .map(|(alias, _)| alias.clone()),
                        );
                        this.close_names(&names, window, cx)
                    });
                }
                crate::note_library::LibraryEvent::Open { key, view } => {
                    this.activate_center(key, cx);
                    let panel: Arc<dyn PanelView> = Arc::new(view.clone());
                    this.put_center_panel(key, panel, window, cx);
                    this.note_panels.insert(key.clone(), view.clone());
                    this.center_order.retain(|name| name != key);
                    this.center_order.push(key.clone());
                    this.restore_note_views(key, view, window, cx);
                    this.record_center_open(key, cx);
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
                        this.pending_install = None;
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
        let updates =
            cx.new(|cx| crate::updates::UpdateView::new(database.read(cx).output.clone(), cx));
        let update_subscription = cx.subscribe_in(
            &updates,
            window,
            |this, _, event: &crate::updates::Install, window, cx| {
                if this.install_launching || this.pending_install.is_some() {
                    crate::feedback::notify(
                        gpui_component::notification::NotificationType::Info,
                        t(cx, "updates.install.preparing"),
                        cx,
                    );
                    return;
                }
                let download = event.0.clone();
                let owner = cx.entity().downgrade();
                crate::feedback::confirm_labels(
                    t(cx, "updates.install.title"),
                    t(cx, "updates.install.confirm"),
                    t(cx, "updates.action.later"),
                    t(cx, "updates.action.install"),
                    window,
                    cx,
                    move |window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.pending_install = Some(download.clone());
                            if this.prepare_close(window, cx) {
                                this.finish_close(window, cx);
                            }
                        });
                    },
                );
            },
        );
        let startup_updates = updates.clone();
        cx.defer_in(window, move |_, window, cx| {
            startup_updates.update(cx, |updates, cx| updates.check(true, window, cx))
        });
        Self {
            notes,
            note_panels: Default::default(),
            note_views: Default::default(),
            restored_note_views: vec![],
            _notes_subscription: notes_subscription,
            _notes_observe: notes_observe,
            pending_note_close: None,
            closing_notes_window: false,
            #[cfg(target_os = "windows")]
            tray: None,
            #[cfg(target_os = "windows")]
            tray_ready: false,
            #[cfg(target_os = "windows")]
            tray_epoch: 0,
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
            table_drafts: Default::default(),
            permission_panels: Default::default(),
            _permission_subscription: permission_subscription,
            pending_table_restore: vec![],
            restored_center: vec![],
            center_sizes: vec![],
            layout_task: None,
            layout_ready: false,
            layout_error: false,
            center_groups: vec![vec!["notebook".into()]],
            center_views: vec![center_tabs.clone()],
            active_center: 0,
            _layout_subscription: layout_subscription,
            center_order: vec!["notebook".into()],
            right_order: vec!["ai-chat".into(), "properties".into(), "schema".into()],
            pending_close: vec![],
            _new_note_subscription: new_note_subscription,
            _tab_subscription: tab_subscription,
            _feedback_subscription: feedback_subscription,
            update_status: cx
                .global::<crate::preferences::Preferences>()
                .load_error
                .clone()
                .unwrap_or_default(),
            updates,
            _update_subscription: update_subscription,
            pending_install: None,
            install_launching: false,
            dock,
            ai,
            settings,
            center_tabs: center_tabs.clone(),
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
        self.put_center_panel("settings", Arc::new(self.settings.clone()), window, cx);
        if !self.center_order.iter().any(|name| name == "settings") {
            self.center_order.push("settings".into());
        }
        self.record_center_open("settings", cx);
        self.settings_open = true;
        window.focus(&self.settings.read(cx).focus_handle(cx));
        cx.notify();
    }

    pub fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.install_launching {
            return false;
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.prepare_close(window, cx)
        }
        #[cfg(target_os = "windows")]
        {
            if self.closing_window || self.closing_notes_window || window.has_active_dialog(cx) {
                return false;
            }
            if cx
                .global::<crate::preferences::Preferences>()
                .general
                .close_to_tray
            {
                self.keep_in_tray(window, cx);
                return false;
            }
            let owner = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, cx| {
                let exit = owner.clone();
                let keep = owner.clone();
                dialog
                    .title(t(cx, "app.close.title"))
                    .child(
                        div()
                            .h_flex()
                            .items_start()
                            .gap_3()
                            .child(Icon::new(IconName::Info))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .child(t(cx, "app.close.description")),
                            ),
                    )
                    .footer(move |_, _, _, cx| {
                        let exit = exit.clone();
                        let keep = keep.clone();
                        vec![
                            crate::feedback::footer_actions()
                                .child(
                                    crate::feedback::close_button(cx)
                                        .label(t(cx, "app.close.cancel")),
                                )
                                .child(
                                    Button::new("app-exit")
                                        .icon(IconName::Close)
                                        .label(t(cx, "app.close.exit"))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = exit.update(cx, |this, cx| {
                                                this.exit_application(window, cx)
                                            });
                                        }),
                                )
                                .child(
                                    Button::new("app-keep-tray")
                                        .primary()
                                        .icon(IconName::PanelBottom)
                                        .label(t(cx, "app.close.tray"))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = keep.update(cx, |this, cx| {
                                                this.keep_in_tray(window, cx)
                                            });
                                        }),
                                ),
                        ]
                    })
            });
            false
        }
    }
    #[cfg(target_os = "windows")]
    fn exit_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tray_epoch = self.tray_epoch.wrapping_add(1);
        self.tray_ready = false;
        self.tray = None;
        crate::windows_tray::restore(window);
        if self.prepare_close(window, cx) {
            self.finish_close(window, cx);
        }
    }
    #[cfg(target_os = "windows")]
    fn remember_close_to_tray(&mut self, cx: &mut Context<Self>) -> bool {
        let mut prefs = cx.global::<crate::preferences::Preferences>().clone();
        if prefs.general.close_to_tray {
            return true;
        }
        if self.ai.read(cx).settings_saving() {
            crate::feedback::error(t(cx, "app.tray.remember_busy"), cx);
            return false;
        }
        prefs.general.close_to_tray = true;
        if prefs.save().is_err() {
            crate::feedback::error(t(cx, "app.tray.remember_failed"), cx);
            return false;
        }
        cx.set_global(prefs);
        self.settings
            .update(cx, |settings, cx| settings.sync_close_to_tray(true, cx));
        true
    }
    #[cfg(target_os = "windows")]
    fn keep_in_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray_ready {
            if self.remember_close_to_tray(cx) {
                crate::windows_tray::hide(window);
            }
            return;
        }
        if self.tray.is_some() {
            return;
        }
        let (tray, events) =
            crate::windows_tray::Tray::start(&t(cx, "app.tray.open"), &t(cx, "app.close.exit"));
        self.tray = Some(tray);
        self.tray_epoch = self.tray_epoch.wrapping_add(1);
        let epoch = self.tray_epoch;
        cx.spawn_in(window, async move |owner, cx| {
            while let Ok(event) = events.recv().await {
                if owner
                    .update_in(cx, |this, window, cx| {
                        if this.tray_epoch != epoch {
                            return;
                        }
                        use crate::windows_tray::Event;
                        match event {
                            Event::Ready => {
                                this.tray_ready = true;
                                if this.remember_close_to_tray(cx) {
                                    crate::windows_tray::hide(window);
                                }
                            }
                            Event::Open => crate::windows_tray::restore(window),
                            Event::Exit => this.exit_application(window, cx),
                            Event::Failed => {
                                this.tray_ready = false;
                                this.tray = None;
                                crate::windows_tray::restore(window);
                                crate::feedback::error(t(cx, "app.tray.failed"), cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn prepare_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self
            .permission_panels
            .values()
            .any(|p| p.read(cx).running())
        {
            crate::feedback::error(t(cx, "accounts.workbench.busy"), cx);
            return false;
        }
        let pending = self
            .permission_panels
            .values()
            .filter(|p| p.read(cx).dirty())
            .cloned()
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            let owner = cx.entity().downgrade();
            crate::feedback::confirm_labels(
                t(cx, "accounts.permissions"),
                t(cx, "accounts.workbench.discard"),
                t(cx, "dbwork.draft.keep"),
                t(cx, "dbwork.draft.discard"),
                window,
                cx,
                move |window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        for p in &pending {
                            p.update(cx, |p, cx| p.discard(cx));
                        }
                        if this.prepare_close(window, cx) {
                            this.finish_close(window, cx);
                        }
                    });
                },
            );
            return false;
        }

        if self.pending_install.is_some()
            && (self.database.read(cx).update_blocked(cx)
                || self
                    .table_panels
                    .values()
                    .any(|table| table.read(cx).update_blocked())
                || self
                    .note_panels
                    .values()
                    .any(|note| note.read(cx).running(cx)))
        {
            self.pending_install = None;
            crate::feedback::error(t(cx, "updates.install.blocked"), cx);
            return false;
        }
        let pending = self
            .table_panels
            .values()
            .filter(|table| table.read(cx).mongo_unsaved())
            .cloned()
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            let owner = cx.entity().downgrade();
            crate::feedback::confirm_labels(
                t(cx, "mongo.document.close_title"),
                t(cx, "mongo.document.close_body"),
                t(cx, "dbwork.draft.keep"),
                t(cx, "dbwork.draft.discard"),
                window,
                cx,
                move |window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        for table in &pending {
                            table.update(cx, |table, cx| table.discard_mongo(cx));
                        }
                        if pending.iter().any(|table| table.read(cx).mongo_unsaved()) {
                            return;
                        }
                        if this.prepare_close(window, cx) {
                            this.finish_close(window, cx);
                        }
                    });
                },
            );
            return false;
        }
        let running = self.notes.read(cx).running(cx);
        if self.pending_install.is_some() && self.notes.read(cx).pending(cx) {
            // The install confirmation explicitly authorizes saving, even with autosave disabled.
            // Running queries and staged results were rejected above; never stop/discard them here.
            self.closing_notes_window = true;
            self.notes.update(cx, |notes, cx| notes.flush(cx));
            return false;
        }
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
                self.finish_close(window, cx);
            }
        }
    }
    fn finish_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(download) = self.pending_install.take() {
            self.install_launching = true;
            let id = uuid::Uuid::new_v4();
            let started = std::time::Instant::now();
            let failure_message = t(cx, "updates.error.install_after_exit").to_string();
            self.updates.update(cx, |updates, cx| {
                updates.log(
                    id,
                    crate::operation_log::Phase::Started,
                    started,
                    0,
                    "-",
                    cx,
                )
            });
            crate::feedback::notify(
                gpui_component::notification::NotificationType::Info,
                t(cx, "updates.install.preparing"),
                cx,
            );
            let work = cx.background_executor().spawn(async move {
                crate::updates::launch_installer_helper(&download, failure_message)
            });
            cx.spawn_in(window, async move |owner, cx| {
                let result = work.await;
                let _ = owner.update_in(cx, |this, window, cx| {
                    this.install_launching = false;
                    match result {
                        Err(_) => {
                            this.updates.update(cx, |updates, cx| {
                                updates.log(
                                    id,
                                    crate::operation_log::Phase::Failed,
                                    started,
                                    0,
                                    "update_install",
                                    cx,
                                )
                            });
                            crate::feedback::error(t(cx, "updates.error.install"), cx);
                        }
                        Ok(mut helper) => {
                            if this.database.read(cx).update_blocked(cx)
                                || this
                                    .table_panels
                                    .values()
                                    .any(|table| table.read(cx).update_blocked())
                                || this
                                    .note_panels
                                    .values()
                                    .any(|note| note.read(cx).running(cx))
                                || this.notes.read(cx).pending(cx)
                                || this.settings.read(cx).pending(cx)
                            {
                                this.updates.update(cx, |updates, cx| {
                                    updates.log(
                                        id,
                                        crate::operation_log::Phase::Cancelled,
                                        started,
                                        0,
                                        "-",
                                        cx,
                                    )
                                });
                                cx.background_executor()
                                    .spawn(async move {
                                        let _ = helper.kill();
                                        let _ = helper.wait();
                                    })
                                    .detach();
                                crate::feedback::error(t(cx, "updates.install.blocked"), cx);
                            } else {
                                // This records a verified handoff, not completion of installation.
                                this.updates.update(cx, |updates, cx| {
                                    updates.log(
                                        id,
                                        crate::operation_log::Phase::Prepared,
                                        started,
                                        0,
                                        "-",
                                        cx,
                                    )
                                });
                                window.remove_window();
                            }
                        }
                    }
                });
            })
            .detach();
            return;
        }
        window.remove_window();
    }
    fn rebuild_tabs(&mut self, center: bool, window: &mut Window, cx: &mut Context<Self>) {
        if center {
            self.rebuild_center(window, cx);
            return;
        }
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
        let requested = names.to_vec();
        let mut normalized = vec![];
        for name in &requested {
            let owner = self
                .note_panels
                .get(name)
                .cloned()
                .or_else(|| self.note_views.get(name).map(|v| v.read(cx).owner.clone()));
            if let Some(owner) = owner {
                let source = owner.read(cx).key.clone();
                let views = self
                    .center_order
                    .iter()
                    .filter(|key| {
                        *key == &source
                            || self
                                .note_views
                                .get(*key)
                                .is_some_and(|v| v.read(cx).owner == owner)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if views.iter().any(|key| !requested.contains(key))
                    && self.close_note_view(name, window, cx)
                {
                    continue;
                }
                if !normalized.contains(&source) {
                    normalized.push(source);
                }
                for alias in views {
                    if !normalized.contains(&alias) {
                        normalized.push(alias);
                    }
                }
            } else if !normalized.contains(name) {
                normalized.push(name.clone());
            }
        }
        let names = normalized.as_slice();
        if names.is_empty() {
            return;
        }

        if names
            .iter()
            .filter_map(|n| self.table_panels.get(n))
            .any(|table| table.read(cx).writing())
        {
            crate::feedback::error(t(cx, "tabs.wait_write"), cx);
            return;
        }
        let staged = names
            .iter()
            .filter_map(|n| self.table_panels.get(n))
            .filter(|t| t.read(cx).staged())
            .cloned()
            .collect::<Vec<_>>();
        if !staged.is_empty() {
            let owner = cx.entity().downgrade();
            let names = names.to_vec();
            crate::feedback::confirm_labels(
                t(cx, "notes.close_title"),
                t(cx, "tabs.close_staged"),
                t(cx, "dbwork.draft.keep"),
                t(cx, "dbwork.draft.discard"),
                window,
                cx,
                move |window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if staged.iter().any(|table| table.read(cx).writing()) {
                            return;
                        }
                        for table in &staged {
                            table.update(cx, |table, cx| table.discard_staged(cx));
                        }
                        this.close_names(&names, window, cx);
                    });
                },
            );
            return;
        }

        let pending = names
            .iter()
            .filter_map(|name| self.table_panels.get(name))
            .filter(|table| table.read(cx).mongo_unsaved())
            .cloned()
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            let owner = cx.entity().downgrade();
            let names = names.to_vec();
            crate::feedback::confirm_labels(
                t(cx, "mongo.document.close_title"),
                t(cx, "mongo.document.close_body"),
                t(cx, "dbwork.draft.keep"),
                t(cx, "dbwork.draft.discard"),
                window,
                cx,
                move |window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        for table in &pending {
                            table.update(cx, |table, cx| table.discard_mongo(cx));
                        }
                        if pending.iter().any(|table| table.read(cx).mongo_unsaved()) {
                            return;
                        }
                        this.close_names(&names, window, cx);
                    });
                },
            );
            return;
        }
        if names
            .iter()
            .filter_map(|n| self.permission_panels.get(n))
            .any(|p| p.read(cx).running())
        {
            return;
        }
        let pending_permissions = names
            .iter()
            .filter_map(|n| self.permission_panels.get(n))
            .filter(|p| p.read(cx).dirty())
            .cloned()
            .collect::<Vec<_>>();
        if !pending_permissions.is_empty() {
            let owner = cx.entity().downgrade();
            let names = names.to_vec();
            crate::feedback::confirm_labels(
                t(cx, "accounts.permissions"),
                t(cx, "accounts.workbench.discard"),
                t(cx, "dbwork.draft.keep"),
                t(cx, "dbwork.draft.discard"),
                window,
                cx,
                move |window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        for p in &pending_permissions {
                            p.update(cx, |p, cx| p.discard(cx));
                        }
                        this.close_names(&names, window, cx);
                    });
                },
            );
            return;
        }
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
                    self.database
                        .read(cx)
                        .discard_message("dbwork.draft.close_body", cx),
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
            let Some(panel) = self.panel_for(name, cx) else {
                continue;
            };
            if self.center_order.contains(name) {
                self.activate_center(name, cx);
                self.center_tabs
                    .update(cx, |tabs, cx| tabs.remove_panel(panel, window, cx));
            } else if self.right_order.contains(name) {
                self.right_tabs
                    .update(cx, |tabs, cx| tabs.remove_panel(panel, window, cx));
            }
        }
        for name in names {
            if let Some(view) = self.note_views.remove(name) {
                view.read(cx)
                    .owner
                    .clone()
                    .update(cx, |note, cx| note.remove_view(name, cx));
            }
            self.restored_note_views
                .retain(|(key, owner)| key != name && owner != name);
            self.permission_panels.remove(name);
            self.table_panels.remove(name);
            self.table_drafts.remove(name);
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
        self.center_groups
            .iter_mut()
            .for_each(|group| group.retain(|name| !names.contains(name)));
        self.center_groups.retain(|group| !group.is_empty());
        self.restored_center = self.center_groups.clone();
        self.rebuild_tabs(true, window, cx);
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
        self.restore_pending_tables(window, cx);
        self.sync_open_notes(cx);
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
                icon_button(
                    "connections",
                    Icon::default().path("icons/database.svg"),
                    "database.navigation.title",
                    cx,
                )
                .selected(left && self.connections)
                .on_click(cx.listener(|this, _, window, cx| this.navigate(true, window, cx))),
            )
            .child(
                icon_button("project", IconName::BookOpen, "notes.title", cx)
                    .selected(left && !self.connections)
                    .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx))),
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
                                .on_click(move |_, window, cx| {
                                    update.update(cx, |this, cx| {
                                        this.updates.update(cx, |updates, cx| {
                                            updates.check(false, window, cx)
                                        });
                                    })
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
