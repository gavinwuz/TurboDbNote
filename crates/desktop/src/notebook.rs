#[path = "note_batch.rs"]
mod batch_ui;
#[path = "note_query.rs"]
mod query_ui;
#[path = "note_toolbar.rs"]
mod toolbar;
use crate::{
    cell::{CellEvent, CellView},
    database::DatabaseView,
    feedback,
    locale::t,
    preferences::Preferences,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarShow},
};
use std::path::PathBuf;
pub(crate) use toolbar::FileAction;
use turbodbn_core::{Cell, CellContent, Note, TaskStatus};
use turbodbn_services::note_store::{self, Entry, Loaded};
use uuid::Uuid;

#[derive(Clone)]
pub enum NoteEvent {
    Changed(Entry),
    Saved {
        entry: Entry,
        previous: PathBuf,
        retired: bool,
    },
    Failed,
    Ready,
}
pub struct NoteView {
    query_filter: Option<Entity<crate::table_view::TableView>>,
    query_filter_subscription: Option<Subscription>,
    pub key: String,
    pub path: PathBuf,
    pub draft: bool,
    note: Note,
    access_mode: turbodbn_services::access_mode::AccessMode,
    default_database: Entity<InputState>,
    default_picker: Entity<
        gpui_component::select::SelectState<
            gpui_component::select::SearchableVec<crate::database::DatabaseChoice>,
        >,
    >,
    default_context: Option<(Option<Uuid>, Vec<String>, String)>,
    batch: Option<batch_ui::Batch>,
    batch_notice: String,
    cells: Vec<Entity<CellView>>,
    connections: Entity<DatabaseView>,
    focus: FocusHandle,
    add_focus: FocusHandle,
    toolbar_scroll: ScrollHandle,
    cell_list: ListState,
    view_lists: std::collections::HashMap<String, ListState>,
    active_views: std::collections::HashSet<String>,
    editor_height_limit: f32,
    cell_observers: std::collections::HashMap<EntityId, Subscription>,
    hash: [u8; 32],
    revision: u64,
    saved_revision: u64,
    deleting: bool,
    saving: bool,
    picking: bool,
    flush_again: bool,
    error: String,
    debounce: Option<Task<()>>,
    save_due: std::time::Instant,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    cell_subscriptions: std::collections::HashMap<EntityId, Subscription>,
    import_subscription: Option<Subscription>,
    _preferences: Subscription,
    _connections_observer: Subscription,
}
impl EventEmitter<PanelEvent> for NoteView {}
impl EventEmitter<NoteEvent> for NoteView {}

// GPUI List registers its wheel listener after painting children, so it runs before
// nested inputs in the bubble phase. Restore its anchor before the next frame only
// when a child consumed the event; at an input boundary the outer list keeps scrolling.
fn bounded_scroll_offset(handle: &ScrollHandle) -> Point<Pixels> {
    let offset = handle.offset();
    let maximum = handle.max_offset();
    point(
        offset.x.clamp(-maximum.width, px(0.)),
        offset.y.clamp(-maximum.height, px(0.)),
    )
}
fn nested_scroll(content: AnyElement, list: ListState, cx: &Context<NoteView>) -> AnyElement {
    let pending = std::rc::Rc::new(std::cell::RefCell::new(
        None::<std::rc::Rc<std::cell::Cell<bool>>>,
    ));
    let before = pending.clone();
    let owner = cx.entity().downgrade();
    div()
        .relative()
        .size_full()
        .min_h_0()
        .min_w_0()
        .child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let before = before.clone();
                    window.on_mouse_event(move |_: &ScrollWheelEvent, phase, _, _| {
                        if phase == DispatchPhase::Bubble
                            && let Some(event) = before.borrow_mut().take()
                        {
                            event.set(false);
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(content)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    // A result/cell can shrink until the entire Note fits. List retains
                    // its logical anchor, so normalize it after current-frame measurement.
                    let maximum = list.max_offset_for_scrollbar().height;
                    let offset = list.scroll_px_offset_for_scrollbar();
                    if offset.y < -maximum || offset.y > px(0.) {
                        let list = list.clone();
                        let owner = owner.clone();
                        cx.defer(move |cx| {
                            let offset = list.scroll_px_offset_for_scrollbar();
                            let maximum = list.max_offset_for_scrollbar().height;
                            let y = offset.y.clamp(-maximum, px(0.));
                            if y != offset.y {
                                list.set_offset_from_scrollbar(point(px(0.), y));
                                let _ = owner.update(cx, |_, cx| cx.notify());
                            }
                        });
                    }
                    let pending = pending.clone();
                    let list = list.clone();
                    let owner = owner.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || !bounds.contains(&event.position) {
                            return;
                        }
                        let handled = std::rc::Rc::new(std::cell::Cell::new(true));
                        *pending.borrow_mut() = Some(handled.clone());
                        let top = list.logical_scroll_top();
                        let offset = list.scroll_px_offset_for_scrollbar();
                        let delta = event.delta.pixel_delta(px(20.));
                        let maximum = list.max_offset_for_scrollbar().height;
                        let inner = owner
                            .read_with(cx, |this, cx| {
                                this.cells
                                    .iter()
                                    .flat_map(|cell| cell.read(cx).note_scroll_handles(cx))
                                    .map(|handle| {
                                        let offset = bounded_scroll_offset(&handle);
                                        let raw = handle.offset();
                                        (handle, offset, raw)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let list = list.clone();
                        let owner = owner.clone();
                        window.defer(cx, move |window, cx| {
                            let moved = inner.iter().any(|(handle, offset, _)| {
                                bounded_scroll_offset(handle) != *offset
                            });
                            let boundary = inner.iter().any(|(handle, offset, raw)| {
                                handle.offset() != *raw && bounded_scroll_offset(handle) == *offset
                            });
                            for (handle, _, _) in &inner {
                                let bounded = bounded_scroll_offset(handle);
                                if bounded != handle.offset() {
                                    handle.set_offset(bounded);
                                }
                            }
                            // Table ScrollableMask consumes wheel deltas even past its end.
                            // Its raw offset changes but the visible (clamped) position does not.
                            if moved || (handled.get() && !boundary) {
                                list.scroll_to(top);
                                let _ = owner.update(cx, |_, cx| cx.notify());
                            } else {
                                // Commit one wheel delta from the event's original offset.
                                // Do not retain List's frame-local accumulated delta across
                                // cached paints; explicitly invalidate the owning Note view.
                                let y = (offset.y + delta.y).clamp(-maximum, px(0.));
                                list.set_offset_from_scrollbar(point(px(0.), y));
                                let _ = owner.update(cx, |this, cx| {
                                    if y != offset.y {
                                        this.focus.focus(window);
                                    }
                                    cx.notify();
                                });
                            }
                        });
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}

impl Focusable for NoteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl NoteView {
    fn single_sql(&self) -> bool {
        self.note.table_query.is_some() && self.cells.len() == 1
    }
    fn permission_help(
        &self,
        advice: crate::note_permissions::Advice,
        origin: EntityId,
        cx: &mut Context<Self>,
    ) {
        use crate::note_permissions::Advice;
        let reason = advice.message(cx);
        let text = crate::locale::tr(
            cx,
            "permissions.note_context",
            &[("note", &self.note.title), ("reason", &reason)],
        );
        let owner = cx.entity().downgrade();
        let revision = self.revision;
        let mode = self.access_mode;
        match advice {
            Advice::Mode {
                required,
                key,
                command,
                ..
            } => {
                let label = crate::locale::tr(
                    cx,
                    "permissions.switch",
                    &[("mode", &t(cx, required.label()))],
                );
                feedback::action_error(
                    text,
                    label,
                    move |_, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.revision != revision || this.access_mode != mode {
                                feedback::error(t(cx, "permissions.expired"), cx);
                                return;
                            }
                            if !cx.global::<Preferences>().database.command_enabled(&key) {
                                this.permission_help(
                                    Advice::Global {
                                        command: command.clone(),
                                    },
                                    origin,
                                    cx,
                                );
                                return;
                            }
                            if this.defaults_locked(cx) {
                                feedback::error(t(cx, "permissions.busy"), cx);
                                return;
                            }
                            this.access_mode = required;
                            for cell in &this.cells {
                                cell.update(cx, |cell, cx| cell.set_access_mode(required, cx));
                            }
                            feedback::success(
                                crate::locale::tr(
                                    cx,
                                    "permissions.switched",
                                    &[("mode", &t(cx, required.label()))],
                                ),
                                cx,
                            );
                            cx.notify();
                        });
                    },
                    cx,
                );
            }
            Advice::Global { .. } => feedback::action_error(
                text,
                t(cx, "settings.sql_policy_open"),
                move |_, cx| {
                    let hub = cx.global::<feedback::Hub>().0.clone();
                    cx.defer(move |cx| hub.update(cx, |_, cx| cx.emit(feedback::OpenSqlSettings)));
                },
                cx,
            ),
            Advice::Connection => {
                let cell = self
                    .cells
                    .iter()
                    .find(|cell| cell.entity_id() == origin)
                    .map(|cell| cell.downgrade());
                feedback::action_error(
                    text,
                    t(cx, "permissions.choose_connection"),
                    move |window, cx| {
                        if let Some(cell) = &cell {
                            let _ =
                                cell.update(cx, |cell, cx| cell.permission_connection(window, cx));
                        }
                    },
                    cx,
                );
            }
            Advice::Unsupported => feedback::error(text, cx),
        }
    }
    pub(super) fn import_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import_sql_replace(None, window, cx);
    }
    pub(super) fn import_sql_replace(
        &mut self,
        replace: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.batch.is_some() || self.active_queries(cx) || self.picking || self.saving {
            return;
        }
        self.picking = true;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t(cx, "script.import")),
        });
        cx.spawn_in(window, async move |owner, cx| {
            let result = paths.await;
            let _ = owner.update_in(cx, |this, window, cx| {
                this.picking = false;
                cx.emit(NoteEvent::Ready);
                if let Ok(Ok(Some(paths))) = result
                    && let Some(source) = paths.first()
                {
                    if !source
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("sql"))
                    {
                        feedback::error(t(cx, "notes.file.only_sql"), cx);
                        return;
                    }
                    let mut note = this.snapshot(cx);
                    if let Some(id) = replace {
                        let Some(index) = this.cells.iter().position(|cell| cell.entity_id() == id)
                        else {
                            return;
                        };
                        note.cells.remove(index);
                    }
                    let path = this.path.clone();
                    let revision = this.revision;
                    let connection = replace
                        .and_then(|id| this.cells.iter().find(|cell| cell.entity_id() == id))
                        .and_then(|cell| match cell.read(cx).snapshot(cx).content {
                            CellContent::SqlScript { connection_id, .. } => connection_id,
                            _ => None,
                        })
                        .or(this.note.default_connection_id);
                    let dialect = connection
                        .and_then(|id| this.connections.read(cx).note_profile(id))
                        .and_then(|profile| turbodbn_services::sql_script::dialect(profile.kind));
                    let output = this.connections.read(cx).output.clone();
                    let importer = cx.new(|cx| {
                        crate::script_import::ImportView::new(
                            source.clone(),
                            path.clone(),
                            note,
                            dialect,
                            output,
                            window,
                            cx,
                        )
                    });
                    this.import_subscription = Some(cx.subscribe_in(
                        &importer,
                        window,
                        move |this, _, event: &crate::script_import::Imported, window, cx| {
                            if this.revision != revision
                                || this.path != path
                                || this.active_queries(cx)
                            {
                                feedback::error(t(cx, "notes.preview_changed"), cx);
                                return;
                            }
                            if let Some(id) = replace {
                                let Some(index) =
                                    this.cells.iter().position(|cell| cell.entity_id() == id)
                                else {
                                    return;
                                };
                                let previous = this.cells[index].read(cx).snapshot(cx);
                                let Some(mut cell) = event.cells.first().cloned() else {
                                    return;
                                };
                                let CellContent::SqlScript {
                                    connection_id: old_connection,
                                    database: old_database,
                                    ..
                                } = previous.content
                                else {
                                    return;
                                };
                                let CellContent::SqlScript {
                                    connection_id,
                                    database,
                                    ..
                                } = &mut cell.content
                                else {
                                    return;
                                };
                                *connection_id = old_connection;
                                *database = old_database;
                                cell.id = previous.id;
                                cell.title = previous.title;
                                this.cell_subscriptions.remove(&id);
                                this.cell_observers.remove(&id);
                                let entity = this.push_cell(cell, window, cx);
                                this.cells.pop();
                                this.cells[index] = entity;
                            } else {
                                for cell in &event.cells {
                                    this.push_cell(cell.clone(), window, cx);
                                }
                            }
                            this.changed(cx);
                            event
                                .accepted
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                        },
                    ));
                    let cancel = importer.downgrade();
                    window.open_dialog(cx, move |dialog, _, cx| {
                        let cancel = cancel.clone();
                        dialog
                            .title(t(cx, "script.import"))
                            .w(px(650.))
                            .overlay_closable(false)
                            .on_cancel(move |_, _, cx| {
                                let _ = cancel.update(cx, |this, cx| this.cancel(cx));
                                true
                            })
                            .child(importer.clone())
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub fn new(
        mut loaded: Loaded,
        path: PathBuf,
        draft: bool,
        connections: Entity<DatabaseView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let default_database = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(cx, "notes.default_database"))
                .default_value(loaded.note.default_database.clone().unwrap_or_default())
        });
        let default_picker = cx.new(|cx| {
            gpui_component::select::SelectState::new(
                gpui_component::select::SearchableVec::new(
                    Vec::<crate::database::DatabaseChoice>::new(),
                ),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let mut subscriptions = vec![
            cx.subscribe(&default_database, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.sync_defaults(cx);
                    this.changed(cx);
                }
            }),
            cx.subscribe_in(
                &default_picker,
                window,
                |this,
                 _,
                 event: &gpui_component::select::SelectEvent<
                    gpui_component::select::SearchableVec<crate::database::DatabaseChoice>,
                >,
                 window,
                 cx| {
                    let gpui_component::select::SelectEvent::Confirm(value) = event;
                    this.default_database.update(cx, |input, cx| {
                        input.set_value(value.clone().unwrap_or_default(), window, cx)
                    });
                },
            ),
        ];
        let connections_observer = cx.observe(&connections, |_, _, cx| cx.notify());
        let add_focus = cx.focus_handle();
        subscriptions.push(cx.on_focus_in(&add_focus, window, |_, _, cx| cx.notify()));
        subscriptions.push(cx.on_focus_out(&add_focus, window, |_, _, _, cx| cx.notify()));
        let observer = cx.observe_global::<Preferences>(|this, cx| {
            this.sync_cell_list(cx);
            if !cx.global::<Preferences>().general.note_autosave {
                this.debounce = None;
            } else if this.dirty() {
                this.schedule(cx);
            }
            cx.notify();
        });
        let cells = std::mem::take(&mut loaded.note.cells);
        let mut this = Self {
            key: format!("note-{}", loaded.note.id),
            path,
            draft,
            query_filter: None,
            query_filter_subscription: None,
            note: loaded.note,
            access_mode: Default::default(),
            default_database,
            default_picker,
            default_context: None,
            batch: None,
            batch_notice: String::new(),
            cells: vec![],
            connections,
            focus: cx.focus_handle(),
            add_focus,
            toolbar_scroll: ScrollHandle::new(),
            cell_list: ListState::new(0, ListAlignment::Top, px(200.)).measure_all(),
            view_lists: Default::default(),
            active_views: Default::default(),
            editor_height_limit: 280.,
            cell_observers: Default::default(),
            hash: loaded.hash,
            revision: 0,
            saved_revision: 0,
            deleting: false,
            saving: false,
            picking: false,
            flush_again: false,
            error: String::new(),
            debounce: None,
            save_due: std::time::Instant::now(),
            save_task: None,
            _subscriptions: subscriptions,
            cell_subscriptions: Default::default(),
            import_subscription: None,
            _preferences: observer,
            _connections_observer: connections_observer,
        };
        this._subscriptions
            .push(cx.on_focus_in(&this.focus.clone(), window, |this, _, cx| {
                crate::tabs::layout(crate::tabs::LayoutAction::Activate(this.key.clone()), cx)
            }));
        for cell in cells {
            this.push_cell(cell, window, cx);
        }
        this.view_lists
            .insert(this.key.clone(), this.cell_list.clone());
        this.active_views.insert(this.key.clone());
        this.sync_cell_list(cx);
        let owner = cx.entity().downgrade();
        this.cell_list.set_scroll_handler(move |_, window, cx| {
            let owner = owner.clone();
            window.defer(cx, move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this
                        .cells
                        .iter()
                        .any(|cell| cell.read(cx).reading_focused(window, cx))
                    {
                        this.focus.focus(window);
                    }
                });
            });
        });
        this
    }
    fn push_cell(
        &mut self,
        cell: Cell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CellView> {
        let entity = cx.new(|cx| {
            CellView::new(
                cell,
                self.connections.clone(),
                self.path.clone(),
                window,
                cx,
            )
        });
        let database = self.default_database.read(cx).value().trim().to_owned();
        entity.update(cx, |cell, cx| {
            cell.set_access_mode(self.access_mode, cx);
            cell.set_defaults(self.note.default_connection_id, database, cx)
        });
        let id = entity.entity_id();
        self.cell_subscriptions.insert(
            id,
            cx.subscribe_in(
                &entity,
                window,
                move |this, _, event, window, cx| match event {
                    CellEvent::PermissionBlocked(advice) => {
                        this.permission_help(advice.clone(), id, cx)
                    }
                    CellEvent::Changed => this.content_changed(cx),
                    CellEvent::ReplaceSqlFile => this.import_sql_replace(Some(id), window, cx),
                    CellEvent::SqlAvailabilityChanged => cx.notify(),
                    CellEvent::SelectTarget => cx.defer_in(window, |this, window, cx| {
                        this.select_script_target(window, cx)
                    }),
                    CellEvent::BatchFinished { token, success } => {
                        this.batch_finished(id, *token, *success, window, cx)
                    }
                    CellEvent::Edit => this.activate_cell(id, window, cx),
                    CellEvent::Insert { after, copy } => {
                        if this.batch.is_some() || this.single_sql() {
                            return;
                        }
                        if this.cells.len() >= note_store::MAX_CELLS {
                            return;
                        }
                        if let Some(index) = this.cells.iter().position(|c| c.entity_id() == id) {
                            let source = this.cells[index].read(cx).snapshot(cx);
                            let new = if *copy {
                                source.duplicate()
                            } else {
                                source.empty_sibling()
                            };
                            let entity = this.push_cell(new, window, cx);
                            this.cells.pop();
                            this.cells
                                .insert(index + usize::from(*after), entity.clone());
                            this.activate_cell(entity.entity_id(), window, cx);
                            this.changed(cx);
                        }
                    }
                    CellEvent::Move(direction) => {
                        if this.batch.is_some() || this.single_sql() {
                            return;
                        }
                        if let Some(index) = this.cells.iter().position(|c| c.entity_id() == id) {
                            let to = (index as i32 + direction)
                                .clamp(0, this.cells.len().saturating_sub(1) as i32)
                                as usize;
                            this.cells.swap(index, to);
                            this.cell_list.scroll_to_reveal_item(to);
                            this.changed(cx);
                        }
                    }
                    CellEvent::Remove => {
                        if this.batch.is_some() || this.single_sql() {
                            return;
                        }
                        let view = cx.entity().downgrade();
                        feedback::confirm(
                            t(cx, "notes.remove_cell"),
                            t(cx, "notes.remove_cell_body"),
                            window,
                            cx,
                            move |_, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.cells.retain(|c| c.entity_id() != id);
                                    this.cell_subscriptions.remove(&id);
                                    this.cell_observers.remove(&id);
                                    this.changed(cx);
                                });
                            },
                        );
                    }
                },
            ),
        );
        self.cell_observers.insert(
            id,
            cx.observe(&entity, move |this, _, cx| {
                if this.single_sql() {
                    return;
                }
                if let Some(index) = this.cells.iter().position(|cell| cell.entity_id() == id) {
                    for list in this.view_lists.values() {
                        let top = list.logical_scroll_top();
                        let viewport = list.viewport_bounds();
                        let visible = list.bounds_for_item(index).is_some_and(|bounds| {
                            bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom()
                        });
                        // Visible items are remeasured by List on every layout. Retain their
                        // height during grid/selection notifications instead of replacing it
                        // with zero between a wheel event and the next frame.
                        if !visible {
                            list.splice_focusable(index..index + 1, [None]);
                            list.scroll_to(top);
                        }
                    }
                    cx.notify();
                }
            }),
        );
        self.cells.push(entity.clone());
        entity
    }
    fn sync_cell_list(&self, cx: &mut Context<Self>) {
        for (index, cell) in self.cells.iter().enumerate() {
            cell.update(cx, |cell, cx| {
                cell.set_query_only(self.single_sql());
                cell.set_editor_height_limit(self.editor_height_limit, cx);
                cell.set_position(index, self.cells.len(), cx);
                cell.set_note_name(&self.note.title, cx);
            });
        }
        for list in self.view_lists.values() {
            let top = list.logical_scroll_top();
            list.splice_focusable(0..list.item_count(), self.cells.iter().map(|_| None));
            list.scroll_to(top);
        }
    }
    fn layout_cell(&self, index: usize, cx: &mut Context<Self>) -> Entity<CellView> {
        let cell = self.cells[index].clone();
        cell.update(cx, |cell, cx| {
            cell.set_split_view(self.active_views.len() > 1, cx);
            cell.set_query_only(self.single_sql());
            cell.set_note_name(&self.note.title, cx);
            cell.set_batch_locked(self.batch.is_some(), cx);
        });
        cell
    }
    fn activate_cell(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let structure_changed = self.cell_list.item_count() != self.cells.len();
        if structure_changed {
            self.sync_cell_list(cx);
        }
        for cell in &self.cells {
            if cell.entity_id() != id {
                cell.update(cx, |cell, cx| cell.set_editing(false, window, cx));
            }
        }
        if let Some(cell) = self.cells.iter().find(|c| c.entity_id() == id) {
            cell.update(cx, |cell, cx| cell.set_editing(true, window, cx));
        }
        if let Some(index) = self.cells.iter().position(|c| c.entity_id() == id) {
            if structure_changed {
                self.cell_list.scroll_to(ListOffset {
                    item_ix: index,
                    offset_in_item: px(0.),
                });
            } else {
                self.cell_list.scroll_to_reveal_item(index);
            }
        }
        cx.notify();
    }
    pub fn display_title(&self, cx: &App) -> SharedString {
        let title = if self.note.title.trim().is_empty() {
            let stem = self.path.file_stem().unwrap_or_default().to_string_lossy();
            if Uuid::parse_str(&stem).is_ok() {
                t(cx, "notes.legacy_title").to_string()
            } else {
                stem.into_owned()
            }
        } else {
            self.note.title.clone()
        };
        let automatic = self.note.title.trim().is_empty()
            || self.note.title.starts_with("note-")
            || self
                .note
                .table_query
                .as_ref()
                .is_some_and(|(_, table)| table == &self.note.title);
        if automatic
            && let Some(profile) = self
                .note
                .default_connection_id
                .and_then(|id| self.connections.read(cx).note_profile(id))
        {
            let address = short_address(&profile.host, profile.port);
            if !address.is_empty() {
                return format!("{title} · {address}").into();
            }
        }
        title.into()
    }
    pub(crate) fn search_identity(&self, cx: &App) -> (String, String, String, bool) {
        let connection = self
            .note
            .default_connection_id
            .and_then(|id| self.connections.read(cx).note_profile(id))
            .map(|p| format!("{} {}:{}", p.name, p.host, p.port))
            .unwrap_or_default();
        (
            self.display_title(cx).to_string(),
            self.note.project.clone(),
            connection,
            self.dirty(),
        )
    }
    pub fn entry(&self, _cx: &App) -> Entry {
        Entry {
            id: self.note.id,
            path: self.path.clone(),
            title: self.note.title.clone(),
            project: self.note.project.clone(),
            draft: self.draft,
            used: note_store::now(),
        }
    }
    pub fn dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
    pub(crate) fn begin_delete(&mut self, cx: &mut Context<Self>) -> Option<[u8; 32]> {
        if self.deleting || self.pending() || self.running(cx) {
            return None;
        }
        self.deleting = true;
        self.debounce = None;
        cx.notify();
        Some(self.hash)
    }
    pub(crate) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.deleting = false;
        cx.notify();
    }
    pub fn pending(&self) -> bool {
        self.dirty() || self.saving || self.picking
    }
    pub fn running(&self, cx: &App) -> bool {
        self.batch.is_some()
            || self
                .cells
                .iter()
                .any(|cell| cell.read(cx).working(cx) || cell.read(cx).result_pending(cx))
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.stop_queries(cx);
        for cell in &self.cells {
            cell.update(cx, |cell, cx| {
                cell.discard_result_changes(cx);
            });
        }
    }
    // Typing changes the document revision, not its layout or index metadata.
    // Only the first dirty transition (or clearing a save error) updates Note chrome.
    fn content_changed(&mut self, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        let refresh = !self.dirty() || !self.error.is_empty();
        self.revision += 1;
        self.error.clear();
        self.schedule(cx);
        if refresh {
            cx.emit(NoteEvent::Changed(self.entry(cx)));
            cx.emit(PanelEvent::LayoutChanged);
            cx.notify();
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        self.sync_cell_list(cx);
        self.revision += 1;
        self.error.clear();
        cx.emit(NoteEvent::Changed(self.entry(cx)));
        cx.emit(PanelEvent::LayoutChanged);
        self.schedule(cx);
        cx.notify();
    }
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if !cx.global::<Preferences>().general.note_autosave || self.picking {
            return;
        }
        self.save_due = std::time::Instant::now() + std::time::Duration::from_millis(600);
        if self.debounce.is_some() {
            return;
        }
        self.debounce = Some(cx.spawn(async move |view, cx| {
            loop {
                let remaining = view
                    .read_with(cx, |view, _| {
                        view.save_due
                            .saturating_duration_since(std::time::Instant::now())
                    })
                    .ok();
                let Some(remaining) = remaining else { return };
                if remaining.is_zero() {
                    break;
                }
                cx.background_executor().timer(remaining).await;
            }
            let _ = view.update(cx, |this, cx| {
                this.debounce = None;
                this.save_now(cx);
            });
        }));
    }
    fn snapshot(&self, cx: &App) -> Note {
        let (mut note, cells) = self.snapshot_deferred(cx);
        note.cells = cells
            .into_iter()
            .map(crate::cell::CellSnapshot::finish)
            .collect();
        note
    }
    fn snapshot_deferred(&self, cx: &App) -> (Note, Vec<crate::cell::CellSnapshot>) {
        let database = self.default_database.read(cx).value().trim().to_string();
        let note = Note {
            schema_version: self.note.schema_version,
            id: self.note.id,
            title: self.note.title.clone(),
            project: self.note.project.clone(),
            default_connection_id: self.note.default_connection_id,
            default_database: (!database.is_empty()).then_some(database),
            table_query: self.note.table_query.clone(),
            table_query_origin: self.note.table_query_origin.clone(),
            cells: vec![],
        };
        let cells = self
            .cells
            .iter()
            .map(|cell| cell.read(cx).snapshot_deferred(cx))
            .collect();
        (note, cells)
    }
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        self.debounce = None;
        if self.picking {
            self.flush_again = true;
            return;
        }
        if self.saving {
            self.flush_again = true;
            return;
        }
        if !self.dirty() {
            return;
        }
        self.save_to(self.path.clone(), false, false, cx);
    }
    fn save_to(&mut self, path: PathBuf, as_new: bool, move_source: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let (mut snapshot, cells) = self.snapshot_deferred(cx);
        let revision = self.revision;
        let hash = if as_new { None } else { Some(self.hash) };
        let previous = self.path.clone();
        let previous_hash = self.hash;
        let retire = as_new && (self.draft || move_source);
        let task_id = Uuid::new_v4();
        self.log(task_id, crate::operation_log::Phase::Started, None, cx);
        self.saving = true;
        cx.emit(PanelEvent::LayoutChanged);
        self.error.clear();
        let written = path.clone();
        let work = cx.background_executor().spawn(async move {
            snapshot.cells = cells
                .into_iter()
                .map(crate::cell::CellSnapshot::finish)
                .collect();
            turbodbn_services::sql_script::copy_resources(&previous, &written, &snapshot)?;
            let hash = note_store::save(&written, &snapshot, hash)?;
            let mut retired = false;
            if retire
                && previous != written
                && (move_source
                    || previous
                        .file_stem()
                        .is_some_and(|s| s == snapshot.id.to_string().as_str()))
                && note_store::load(&previous).is_ok_and(|v| v.hash == previous_hash)
            {
                retired = std::fs::remove_file(&previous).is_ok();
            }
            Ok::<_, String>((hash, previous, retired))
        });
        self.save_task = Some(cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |this, cx| {
                this.saving = false;
                cx.emit(PanelEvent::LayoutChanged);
                match result {
                    Ok((hash, previous, retired)) => {
                        if move_source && !retired && previous != path {
                            feedback::error(t(cx, "notes.rename_partial"), cx);
                        }
                        this.path = path;
                        for cell in &this.cells {
                            cell.update(cx, |cell, cx| cell.set_note_path(this.path.clone(), cx));
                        }
                        this.hash = hash;
                        this.saved_revision = revision;
                        cx.emit(PanelEvent::LayoutChanged);
                        if as_new {
                            this.draft = false;
                        }
                        this.log(task_id, crate::operation_log::Phase::Succeeded, None, cx);
                        cx.emit(NoteEvent::Saved {
                            entry: this.entry(cx),
                            previous,
                            retired,
                        });
                        if this.dirty()
                            && (this.flush_again
                                || cx.global::<Preferences>().general.note_autosave)
                        {
                            this.flush_again = false;
                            let owner = cx.entity().downgrade();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |this, cx| this.save_now(cx));
                            });
                        }
                    }
                    Err(error) => {
                        this.error = error.clone();
                        this.log(
                            task_id,
                            crate::operation_log::Phase::Failed,
                            Some(&error),
                            cx,
                        );
                        feedback::error(error, cx);
                        cx.emit(NoteEvent::Failed);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.picking || self.cells.iter().any(|cell| cell.read(cx).indexing(cx)) {
            return;
        }
        self.picking = true;
        let directory = if self.draft {
            dirs::document_dir().unwrap_or_else(|| self.path.parent().unwrap().to_path_buf())
        } else {
            self.path.parent().unwrap().to_path_buf()
        };
        let name = if toolbar::valid_name(&self.note.title) {
            format!("{}.note", self.note.title)
        } else {
            format!("note-{}.note", &self.note.id.simple().to_string()[..6])
        };
        let selected = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn_in(window, async move |view, cx| {
            let result = selected.await;
            let _ = view.update(cx, |this, cx| {
                this.picking = false;
                if let Ok(Ok(Some(mut path))) = result {
                    if path.extension().is_none() {
                        path.set_extension("note");
                    }
                    this.note.title = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    this.changed(cx);
                    this.debounce = None;
                    this.save_to(path, true, false, cx);
                } else if this.flush_again {
                    this.flush_again = false;
                    this.save_now(cx);
                } else if this.dirty() {
                    this.schedule(cx);
                }
                cx.emit(NoteEvent::Ready);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn log(
        &self,
        id: Uuid,
        phase: crate::operation_log::Phase,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(output) = self.connections.read(cx).output.clone() {
            let mut record =
                crate::operation_log::Record::new(id, crate::operation_log::Kind::Note, phase);
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            output.update(cx, |log, cx| log.operation(record, cx));
        }
    }
}
impl Panel for NoteView {
    fn zoomable(&self, _: &App) -> Option<gpui_component::dock::PanelControl> {
        None
    }
    fn dropdown_menu(
        &mut self,
        menu: gpui_component::menu::PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        crate::tabs::opened_notes(menu, &self.key, window, cx)
    }

    fn toolbar_buttons(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<gpui_component::button::Button>> {
        Some(crate::tabs::new_note_button(self.key.clone(), cx))
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if active {
            feedback::ddl_context(
                feedback::DdlContext {
                    note: None,
                    candidates: vec![],
                    origin: None,
                    scope: None,
                    object: None,
                    open: false,
                },
                cx,
            );
        }
        self.set_view_active(&self.key.clone(), active, cx);
    }

    fn panel_name(&self) -> &'static str {
        "note-editor"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_tab_for(&self.key, cx)
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}
impl NoteView {
    fn render_add_cells(
        &mut self,
        list: ListState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut actions = div()
            .id("note-add-cells")
            .h_flex()
            .justify_center()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap_1()
            .h(px(32.))
            .track_focus(&self.add_focus)
            .tab_index(0);
        for (kind, key) in toolbar::cell_types() {
            actions = actions.child(
                Button::new(("add-note-cell", kind))
                    .ghost()
                    .small()
                    .child(toolbar::cell_icon(kind, cx))
                    .label(t(
                        cx,
                        match kind {
                            0 => "notes.quick.sql",
                            1 => "notes.quick.markdown",
                            2 => "notes.quick.task",
                            _ => "notes.quick.sql_file",
                        },
                    ))
                    .tooltip(t(cx, key))
                    .disabled(self.batch.is_some() || self.cells.len() >= note_store::MAX_CELLS)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_cell(kind, window, cx);
                    })),
            );
        }
        actions
            .child(div().w(px(8.)))
            .child(
                Button::new("note-scroll-top")
                    .ghost()
                    .small()
                    .icon(IconName::ArrowUp)
                    .tooltip(t(cx, "notes.scroll_top"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        list.scroll_to(ListOffset {
                            item_ix: 0,
                            offset_in_item: px(0.),
                        });
                        this.focus.focus(window);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
impl NoteView {
    fn render_content(
        &mut self,
        view_key: String,
        list: ListState,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.deleting {
            return div()
                .p_3()
                .child(t(cx, "notes.deleting"))
                .into_any_element();
        }
        let content = if self.single_sql() {
            div()
                .v_flex()
                .size_full()
                .min_h_0()
                .child(self.layout_cell(0, cx))
                .into_any_element()
        } else {
            // Refill invalidated item heights, including offscreen cells. Cached
            // items are reused, so normal input still lays out only visible cells.
            gpui::list(
                list.clone().measure_all(),
                cx.processor(|this, index, _window, cx| {
                    div()
                        .pb_3()
                        .w_full()
                        .min_w_0()
                        .child(this.layout_cell(index, cx))
                        .into_any_element()
                }),
            )
            .size_full()
            .min_w_0()
            .min_h_0()
            .into_any_element()
        };
        div()
            .id(SharedString::from(view_key.clone()))
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .track_focus(&focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let list = list.clone();
                    move |this, _, _, cx| {
                        this.cell_list = list.clone();
                        crate::tabs::layout(
                            crate::tabs::LayoutAction::Activate(view_key.clone()),
                            cx,
                        );
                    }
                }),
            )
            .p_3()
            .gap_2()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.control
                    && event.keystroke.key.eq_ignore_ascii_case("s")
                {
                    this.save_now(cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.render_toolbar(window, cx))
            .when(self.single_sql(), |body| {
                body.child(
                    Button::new("query-note-filter")
                        .ghost()
                        .icon(IconName::Search)
                        .label(t(cx, "query_note.filter"))
                        .disabled(self.batch.is_some())
                        .on_click(cx.listener(|this, _, w, cx| this.open_query_filter(w, cx))),
                )
            })
            .child(
                div()
                    .id("note-cell-scroll")
                    .v_flex()
                    .w_full()
                    .overflow_hidden()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .pl_3()
                    .pr_3()
                    .relative()
                    .child({
                        let owner = cx.entity().downgrade();
                        canvas(
                            move |bounds, _, cx| {
                                let height = (f32::from(bounds.size.height) - 80.).max(82.).floor();
                                if owner
                                    .read_with(cx, |this, _| {
                                        (this.editor_height_limit - height).abs() >= 1.
                                    })
                                    .unwrap_or(false)
                                {
                                    let owner = owner.clone();
                                    cx.defer(move |cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.editor_height_limit = height;
                                            for cell in &this.cells {
                                                cell.update(cx, |cell, cx| {
                                                    cell.set_editor_height_limit(height, cx)
                                                });
                                            }
                                            cx.notify();
                                        });
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                    })
                    .child(if self.single_sql() {
                        content
                    } else {
                        nested_scroll(content, list.clone(), cx)
                    })
                    .when(!self.single_sql(), |body| {
                        body.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(12.))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, _| this.focus.focus(window)),
                                )
                                .child(
                                    Scrollbar::vertical(&list)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        )
                    }),
            )
            .when(!self.single_sql(), |body| {
                body.child(self.render_add_cells(list.clone(), window, cx))
            })
            .into_any_element()
    }
}

fn short_address(host: &str, port: u16) -> String {
    if host.is_empty() {
        return String::new();
    }
    let host = host
        .parse::<std::net::Ipv4Addr>()
        .map(|ip| {
            let b = ip.octets();
            format!("{}.{}", b[2], b[3])
        })
        .unwrap_or_else(|_| {
            if host.contains(':') {
                format!("[{host}]")
            } else {
                host.to_owned()
            }
        });
    format!("{host}:{port}")
}

impl Render for NoteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self
            .view_lists
            .get(&self.key)
            .cloned()
            .unwrap_or_else(|| self.cell_list.clone());
        self.render_content(self.key.clone(), list, self.focus.clone(), window, cx)
    }
}

/// A viewport owns no document, save worker or database task.
/// Closing it only drops its navigation state; the NoteLibrary remains the sole owner.
pub(crate) struct NotePane {
    pub key: String,
    pub owner: Entity<NoteView>,
    list: ListState,
    focus: FocusHandle,
    _observe: Subscription,
}
impl NotePane {
    pub fn new(key: String, owner: Entity<NoteView>, cx: &mut Context<Self>) -> Self {
        let list = owner.update(cx, |note, _| {
            let list = ListState::new(note.cells.len(), ListAlignment::Top, px(200.)).measure_all();
            list.scroll_to(note.cell_list.logical_scroll_top());
            note.view_lists.insert(key.clone(), list.clone());
            note.active_views.insert(key.clone());
            list
        });
        let observe = cx.observe(&owner, |_, _, cx| cx.notify());
        Self {
            key,
            owner,
            list,
            focus: cx.focus_handle(),
            _observe: observe,
        }
    }
}
impl EventEmitter<PanelEvent> for NotePane {}
impl Focusable for NotePane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for NotePane {
    fn zoomable(&self, _: &App) -> Option<gpui_component::dock::PanelControl> {
        None
    }
    fn panel_name(&self) -> &'static str {
        "note-viewport"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |note, cx| note.render_tab_for(&self.key, cx))
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        Some(crate::tabs::new_note_button(self.key.clone(), cx))
    }
    fn dropdown_menu(
        &mut self,
        menu: gpui_component::menu::PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        {
            let key = self.owner.read(cx).key.clone();
            crate::tabs::opened_notes(menu, &key, window, cx)
        }
    }
    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.owner
            .update(cx, |note, cx| note.set_view_active(&self.key, active, cx));
    }
}
impl Render for NotePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner.update(cx, |note, cx| {
            note.render_content(
                self.key.clone(),
                self.list.clone(),
                self.focus.clone(),
                window,
                cx,
            )
        })
    }
}
impl NoteView {
    pub(crate) fn remove_view(&mut self, key: &str, cx: &mut Context<Self>) {
        self.view_lists.remove(key);
        if let Some(primary) = self.view_lists.get(&self.key) {
            self.cell_list = primary.clone();
        }
        self.set_view_active(key, false, cx);
    }
    fn set_view_active(&mut self, key: &str, active: bool, cx: &mut Context<Self>) {
        if active {
            self.active_views.insert(key.into());
        } else {
            self.active_views.remove(key);
        }
        let active = !self.active_views.is_empty();
        for cell in &self.cells {
            cell.update(cx, |cell, cx| {
                cell.set_note_active(active, cx);
                cell.set_split_view(self.active_views.len() > 1, cx);
            });
        }
    }
}

impl NoteView {
    pub(crate) fn promote_view(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(list) = self.view_lists.remove(key) {
            self.cell_list = list.clone();
            self.view_lists.insert(self.key.clone(), list);
        }
        if self.active_views.remove(key) {
            self.active_views.insert(self.key.clone());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod title_tests {
    #[test]
    fn short_addresses_keep_port_and_handle_non_ipv4_hosts() {
        assert_eq!(super::short_address("172.24.140.38", 3306), "140.38:3306");
        assert_eq!(
            super::short_address("db.example.com", 5432),
            "db.example.com:5432"
        );
        assert_eq!(super::short_address("::1", 3306), "[::1]:3306");
        assert_eq!(super::short_address("", 3306), "");
    }
}
