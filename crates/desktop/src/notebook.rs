#[path = "note_batch.rs"]
mod batch_ui;
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
    ActiveTheme, Disableable, IconName, StyledExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
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
    pub key: String,
    pub path: PathBuf,
    pub draft: bool,
    note: Note,
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
    add_hover: bool,
    scroll: ScrollHandle,
    hash: [u8; 32],
    revision: u64,
    saved_revision: u64,
    saving: bool,
    picking: bool,
    flush_again: bool,
    error: String,
    debounce: Option<Task<()>>,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    cell_subscriptions: std::collections::HashMap<EntityId, Subscription>,
    _preferences: Subscription,
    _connections_observer: Subscription,
}
impl EventEmitter<PanelEvent> for NoteView {}
impl EventEmitter<NoteEvent> for NoteView {}
impl Focusable for NoteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl NoteView {
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
            if !cx.global::<Preferences>().general.note_autosave {
                this.debounce = None;
            } else if this.dirty() {
                this.schedule(cx);
            }
            cx.notify();
        });
        let cells = std::mem::take(&mut loaded.note.cells);
        let mut this = Self {
            key: format!("note-{}", Uuid::new_v4()),
            path,
            draft,
            note: loaded.note,
            default_database,
            default_picker,
            default_context: None,
            batch: None,
            batch_notice: String::new(),
            cells: vec![],
            connections,
            focus: cx.focus_handle(),
            add_focus,
            add_hover: false,
            scroll: ScrollHandle::default(),
            hash: loaded.hash,
            revision: 0,
            saved_revision: 0,
            saving: false,
            picking: false,
            flush_again: false,
            error: String::new(),
            debounce: None,
            save_task: None,
            _subscriptions: subscriptions,
            cell_subscriptions: Default::default(),
            _preferences: observer,
            _connections_observer: connections_observer,
        };
        for cell in cells {
            this.push_cell(cell, window, cx);
        }
        this
    }
    fn push_cell(
        &mut self,
        cell: Cell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CellView> {
        let entity = cx.new(|cx| CellView::new(cell, self.connections.clone(), window, cx));
        let database = self.default_database.read(cx).value().trim().to_owned();
        entity.update(cx, |cell, cx| {
            cell.set_defaults(self.note.default_connection_id, database, cx)
        });
        let id = entity.entity_id();
        self.cell_subscriptions.insert(
            id,
            cx.subscribe_in(
                &entity,
                window,
                move |this, _, event, window, cx| match event {
                    CellEvent::Changed => this.changed(cx),
                    CellEvent::BatchFinished { token, success } => {
                        this.batch_finished(id, *token, *success, window, cx)
                    }
                    CellEvent::Edit => this.activate_cell(id, window, cx),
                    CellEvent::Insert { after, copy } => {
                        if this.batch.is_some() {
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
                        if this.batch.is_some() {
                            return;
                        }
                        if let Some(index) = this.cells.iter().position(|c| c.entity_id() == id) {
                            let to = (index as i32 + direction)
                                .clamp(0, this.cells.len().saturating_sub(1) as i32)
                                as usize;
                            this.cells.swap(index, to);
                            this.scroll.scroll_to_item(to);
                            this.changed(cx);
                        }
                    }
                    CellEvent::Remove => {
                        if this.batch.is_some() {
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
                                    this.changed(cx);
                                });
                            },
                        );
                    }
                },
            ),
        );
        self.cells.push(entity.clone());
        entity
    }
    fn activate_cell(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        for cell in &self.cells {
            if cell.entity_id() != id {
                cell.update(cx, |cell, cx| cell.set_editing(false, window, cx));
            }
        }
        if let Some(cell) = self.cells.iter().find(|c| c.entity_id() == id) {
            cell.update(cx, |cell, cx| cell.set_editing(true, window, cx));
        }
        if let Some(index) = self.cells.iter().position(|c| c.entity_id() == id) {
            self.scroll.scroll_to_item(index);
        }
        cx.notify();
    }
    pub fn display_title(&self, cx: &App) -> SharedString {
        let title: SharedString = self.note.title.clone().into();
        if !title.trim().is_empty() {
            return title;
        }
        if !self.draft {
            return self
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
                .into();
        }
        crate::locale::tr(
            cx,
            "notes.untitled",
            &[("id", &self.note.id.simple().to_string()[..6])],
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
    fn changed(&mut self, cx: &mut Context<Self>) {
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
        self.debounce = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(600))
                .await;
            let _ = view.update(cx, |this, cx| {
                this.debounce = None;
                this.save_now(cx);
            });
        }));
    }
    fn snapshot(&self, cx: &App) -> Note {
        let mut note = self.note.clone();
        let database = self.default_database.read(cx).value().trim().to_string();
        note.default_database = (!database.is_empty()).then_some(database);
        note.cells = self
            .cells
            .iter()
            .map(|cell| cell.read(cx).snapshot(cx))
            .collect();
        note
    }
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
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
        let snapshot = self.snapshot(cx);
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
        if self.saving || self.picking {
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
    fn panel_name(&self) -> &'static str {
        "note-editor"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_tab(cx)
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}
impl Render for NoteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut actions = div()
            .id("note-add-cells")
            .h_flex()
            .justify_center()
            .gap_2()
            .h(px(40.))
            .track_focus(&self.add_focus)
            .tab_index(0)
            .opacity(
                if self.cells.is_empty()
                    || self.add_hover
                    || self.add_focus.contains_focused(window, cx)
                {
                    1.
                } else {
                    0.
                },
            )
            .on_hover(cx.listener(|this, hovered, _, cx| {
                this.add_hover = *hovered;
                cx.notify();
            }));
        for (kind, key) in toolbar::cell_types() {
            actions = actions.child(
                Button::new(("add-note-cell", kind))
                    .ghost()
                    .child(toolbar::cell_icon(kind, cx))
                    .tooltip(t(cx, key))
                    .disabled(self.batch.is_some() || self.cells.len() >= note_store::MAX_CELLS)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_cell(kind, window, cx);
                    })),
            );
        }
        div()
            .id(SharedString::from(self.key.clone()))
            .v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus)
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
            .child(
                div()
                    .id("note-cell-scroll")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .v_flex()
                    .gap_3()
                    .pl_3()
                    .children(self.cells.iter().enumerate().map(|(index, cell)| {
                        cell.update(cx, |cell, cx| {
                            cell.set_position(index, self.cells.len(), cx);
                            cell.set_batch_locked(self.batch.is_some(), cx);
                        });
                        cell.clone()
                    }))
                    .child(actions),
            )
    }
}
