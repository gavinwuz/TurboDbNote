use crate::{
    database::DatabaseView,
    feedback,
    locale::t,
    notebook::{NoteEvent, NoteView},
    preferences::Preferences,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use turbodbn_core::{Cell, CellContent, Note};
use turbodbn_services::note_store::{self, Entry, Index, Loaded};

pub enum LibraryEvent {
    Removed { key: String },
    Open { key: String, view: Entity<NoteView> },
    Settled { key: String, success: bool },
}
pub struct NoteLibrary {
    database: Entity<DatabaseView>,
    index: Index,
    index_path: PathBuf,
    directory: PathBuf,
    documents: HashMap<String, Entity<NoteView>>,
    subscriptions: HashMap<String, Subscription>,
    search: Entity<InputState>,
    collapsed: HashSet<String>,
    deleting: HashSet<PathBuf>,
    loading: bool,
    opening: bool,
    pending_file_action: Option<(PathBuf, crate::notebook::FileAction)>,
    ready: bool,
    error: String,
    writer: async_channel::Sender<()>,
    index_queue: std::sync::Arc<std::sync::Mutex<Option<(u64, Index)>>>,
    index_revision: u64,
    index_saved: u64,
    _writer: Task<()>,
    _search: Subscription,
    _database: Subscription,
    _settings: Subscription,
}
impl EventEmitter<LibraryEvent> for NoteLibrary {}
impl NoteLibrary {
    pub fn directory(cx: &App) -> PathBuf {
        let configured = &cx.global::<Preferences>().general.note_directory;
        if !configured.trim().is_empty() {
            return PathBuf::from(configured);
        }
        Self::default_directory()
    }
    pub fn default_directory() -> PathBuf {
        Preferences::path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("saved")))
            .unwrap_or_else(|| std::env::temp_dir().join("TurboDBNote/saved"))
    }
    pub fn new(
        database: Entity<DatabaseView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = Preferences::path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("note-library.json")))
            .unwrap_or_else(|| std::env::temp_dir().join("TurboDBNote/note-library.json"));
        let directory = Self::directory(cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "notes.search")));
        let (sender, receiver) = async_channel::bounded::<()>(1);
        let destination = path.clone();
        let pending = std::sync::Arc::new(std::sync::Mutex::new(None::<(u64, Index)>));
        let queue = pending.clone();
        let writer = cx.spawn(async move |view, cx| {
            while receiver.recv().await.is_ok() {
                let Some((revision, index)) = queue.lock().unwrap().take() else {
                    continue;
                };
                let destination = destination.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        crate::app_store::save(
                            &destination,
                            "note-library",
                            &index,
                            note_store::MAX_NOTE_BYTES as usize,
                        )
                    })
                    .await;
                let _ = view.update(cx, |this, cx| {
                    match result {
                        Ok(()) => {
                            this.index_saved = revision;
                            cx.emit(LibraryEvent::Settled {
                                key: String::new(),
                                success: true,
                            });
                        }
                        Err(error) => {
                            this.error = error.clone();
                            feedback::error(error, cx);
                            cx.emit(LibraryEvent::Settled {
                                key: String::new(),
                                success: false,
                            });
                        }
                    }
                    cx.notify();
                });
            }
        });
        let this = Self {
            database: database.clone(),
            index: Index {
                version: 1,
                ..Default::default()
            },
            index_path: path,
            directory,
            documents: HashMap::new(),
            subscriptions: HashMap::new(),
            search: search.clone(),
            collapsed: HashSet::new(),
            deleting: HashSet::new(),
            loading: false,
            opening: false,
            pending_file_action: None,
            ready: false,
            error: String::new(),
            writer: sender,
            index_queue: pending,
            index_revision: 0,
            index_saved: 0,
            _writer: writer,
            _search: cx.observe(&search, |_, _, cx| cx.notify()),
            _database: cx.observe(&database, |_, _, cx| cx.notify()),
            _settings: cx.observe_global::<Preferences>(|this, cx| {
                if this.directory != Self::directory(cx) {
                    let owner = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
                cx.notify();
            }),
        };
        cx.defer_in(window, |this, window, cx| this.load(window, cx));
        this
    }
    fn persist(&mut self) {
        if self.ready {
            self.index_revision += 1;
            *self.index_queue.lock().unwrap() = Some((self.index_revision, self.index.clone()));
            let _ = self.writer.try_send(());
        }
    }
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error.clear();
        let path = self.index_path.clone();
        let directory = Self::directory(cx);
        let requested = directory.clone();
        self.directory = requested.clone();
        let initial = !self.ready;
        let snapshot = self.index.clone();
        let work = cx.background_executor().spawn(async move {
            let mut index = if initial {
                crate::app_store::load(
                    &path,
                    "note-library",
                    note_store::MAX_NOTE_BYTES as usize,
                    |index: &Index| index.version == 1 && index.entries.len() <= 2000,
                )?
            } else {
                snapshot
            };
            // Keep old draft roots discoverable; never move or delete existing documents.
            if let Some(base) = path.parent() {
                let legacy = base.join("notes/drafts");
                if legacy.is_dir() && !index.roots.contains(&legacy) {
                    index.roots.push(legacy);
                }
            }
            note_store::discover(&mut index, &directory)?;
            let opened = if initial {
                index
                    .open
                    .iter()
                    .filter_map(|path| index.entries.iter().find(|e| e.path == *path).cloned())
                    .take(16)
                    .map(|entry| {
                        let loaded = note_store::load(&entry.path);
                        (entry, loaded)
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };
            Ok::<_, String>((index, opened))
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.loading = false;
                    match result {
                        Ok((mut index, opened)) => {
                            if !initial {
                                // Live edits/open/close may have happened while discovery ran.
                                for entry in &this.index.entries {
                                    index.entries.retain(|e| e.path != entry.path);
                                    index.entries.push(entry.clone());
                                }
                                index.open = this.index.open.clone();
                            }
                            for entry in &index.entries {
                                this.database.update(cx, |db, cx| {
                                    db.ensure_note_project(&entry.project, cx)
                                });
                            }
                            this.index = index;
                            this.directory = requested;
                            this.ready = true;
                            this.persist();
                            for (entry, loaded) in opened {
                                match loaded {
                                    Ok(loaded) => {
                                        this.install(entry.path, entry.draft, loaded, window, cx)
                                    }
                                    Err(error) => feedback::error(error, cx),
                                }
                            }
                        }
                        Err(error) => {
                            this.error = error.clone();
                            feedback::error(error, cx);
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
    fn install(
        &mut self,
        path: PathBuf,
        draft: bool,
        loaded: Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((key, view)) = self.documents.iter().find(|(_, v)| v.read(cx).path == path) {
            cx.emit(LibraryEvent::Open {
                key: key.clone(),
                view: view.clone(),
            });
            return;
        }
        if self.documents.len() >= 16 {
            feedback::error(t(cx, "notes.open_limit"), cx);
            return;
        }

        if draft
            && let Some(parent) = path.parent()
            && !self.index.roots.contains(&parent.to_path_buf())
        {
            self.index.roots.push(parent.to_path_buf());
        }
        let view =
            cx.new(|cx| NoteView::new(loaded, path, draft, self.database.clone(), window, cx));
        let mut key = view.read(cx).key.clone();
        if self.documents.contains_key(&key) {
            key = format!("{key}:{}", view.read(cx).path.to_string_lossy());
            view.update(cx, |view, _| view.key = key.clone());
        }

        let subscribed_key = key.clone();
        let subscription = cx.subscribe(&view, move |this, _, event: &NoteEvent, cx| match event {
            NoteEvent::Changed(entry) => {
                this.update_entry(entry.clone());
                cx.notify();
            }
            NoteEvent::Saved {
                entry,
                previous,
                retired,
            } => {
                if *retired {
                    this.index.entries.retain(|e| &e.path != previous);
                }
                for path in &mut this.index.open {
                    if path == previous {
                        *path = entry.path.clone();
                    }
                }
                this.update_entry(entry.clone());
                this.database
                    .update(cx, |db, cx| db.ensure_note_project(&entry.project, cx));
                this.persist();
                cx.emit(LibraryEvent::Settled {
                    key: subscribed_key.clone(),
                    success: true,
                });
                cx.notify();
            }
            NoteEvent::Ready => {
                cx.emit(LibraryEvent::Settled {
                    key: subscribed_key.clone(),
                    success: true,
                });
            }
            NoteEvent::Failed => {
                cx.emit(LibraryEvent::Settled {
                    key: subscribed_key.clone(),
                    success: false,
                });
            }
        });
        let entry = view.read(cx).entry(cx);
        if !self.index.open.contains(&entry.path) {
            self.index.open.push(entry.path.clone());
        }
        self.update_entry(entry);
        self.subscriptions.insert(key.clone(), subscription);
        self.documents.insert(key.clone(), view.clone());
        self.persist();
        cx.emit(LibraryEvent::Open {
            key,
            view: view.clone(),
        });
        if self
            .pending_file_action
            .as_ref()
            .is_some_and(|(path, _)| path == &view.read(cx).path)
        {
            let (_, action) = self.pending_file_action.take().unwrap();
            cx.defer_in(window, move |_, window, cx| {
                view.update(cx, |view, cx| view.file_action(action, window, cx))
            });
        }
        cx.notify();
    }
    pub(crate) fn file_action(
        &mut self,
        path: PathBuf,
        action: crate::notebook::FileAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.deleting.contains(&path) {
            return;
        }
        if matches!(action, crate::notebook::FileAction::Reveal) {
            crate::feedback::reveal_parent(path, cx);
            return;
        }
        if let Some(view) = self
            .documents
            .values()
            .find(|v| v.read(cx).path == path)
            .cloned()
        {
            view.update(cx, |view, cx| view.file_action(action, window, cx));
        } else if !self.opening && self.ready && !self.loading {
            self.pending_file_action = Some((path.clone(), action));
            self.open_path(path, window, cx);
        }
    }
    fn delete_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.deleting.contains(&path) || self.opening || self.loading {
            return;
        }
        let Some(entry) = self.index.entries.iter().find(|e| e.path == path) else {
            return;
        };
        let name = entry_title(entry, cx);
        let owner = cx.entity().downgrade();
        feedback::confirm(
            t(cx, "notes.delete_file"),
            crate::locale::tr(cx, "notes.delete_confirm", &[("name", &name)]),
            window,
            cx,
            move |window, cx| {
                let path = path.clone();
                let _ = owner.update(cx, |this, cx| this.archive_note(path, window, cx));
            },
        );
    }
    fn archive_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.deleting.contains(&path) || self.opening || self.loading {
            return;
        }
        let opened = self
            .documents
            .iter()
            .find(|(_, v)| v.read(cx).path == path)
            .map(|(key, v)| (key.clone(), v.clone()));
        let expected = if let Some((_, view)) = &opened {
            let Some(hash) = view.update(cx, |v, cx| v.begin_delete(cx)) else {
                feedback::error(t(cx, "notes.delete_busy"), cx);
                return;
            };
            Some(hash)
        } else {
            None
        };
        self.deleting.insert(path.clone());
        let task_id = uuid::Uuid::new_v4();
        let started = std::time::Instant::now();
        let output = self.database.read(cx).output.clone();
        if let Some(output) = &output {
            output.update(cx, |log, cx| {
                log.operation(
                    crate::operation_log::Record::new(
                        task_id,
                        crate::operation_log::Kind::Note,
                        crate::operation_log::Phase::Started,
                    ),
                    cx,
                )
            });
        }
        let source = path.clone();
        let work = cx
            .background_executor()
            .spawn(async move { note_store::archive(&source, expected) });
        cx.spawn_in(window, async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                this.deleting.remove(&path);
                let success = result.is_ok();
                if let Some(output) = &output {
                    let mut record = crate::operation_log::Record::new(
                        task_id,
                        crate::operation_log::Kind::Note,
                        if success {
                            crate::operation_log::Phase::Succeeded
                        } else {
                            crate::operation_log::Phase::Failed
                        },
                    );
                    record.elapsed_ms = started.elapsed().as_millis() as u64;
                    if let Err(error) = &result {
                        record.failure = crate::operation_log::failure_class(error);
                    }
                    output.update(cx, |log, cx| log.operation(record, cx));
                }
                match result {
                    Ok(_) => {
                        this.index.entries.retain(|e| e.path != path);
                        this.index.open.retain(|p| p != &path);
                        if let Some((key, _)) = &opened {
                            this.documents.remove(key);
                            this.subscriptions.remove(key);
                            cx.emit(LibraryEvent::Removed { key: key.clone() });
                        }
                        this.persist();
                        feedback::success(t(cx, "notes.deleted"), cx);
                    }
                    Err(error) => {
                        if let Some((_, view)) = &opened {
                            view.update(cx, |v, cx| v.cancel_delete(cx));
                        }
                        feedback::error(error, cx);
                    }
                }
                cx.emit(LibraryEvent::Settled {
                    key: String::new(),
                    success,
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn update_entry(&mut self, entry: Entry) {
        if let Some(old) = self.index.entries.iter_mut().find(|e| e.path == entry.path) {
            *old = entry;
        } else {
            self.index.entries.push(entry);
        }
    }
    pub fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_note(None, window, cx);
    }
    pub fn new_table_query(
        &mut self,
        target: crate::database::OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            target.kind,
            turbodbn_services::DatabaseKind::MySql
                | turbodbn_services::DatabaseKind::PostgreSql
                | turbodbn_services::DatabaseKind::Sqlite
        ) {
            feedback::error(t(cx, "query_note.unsupported"), cx);
            return;
        }
        if let Some((key, view)) = self
            .documents
            .iter()
            .find(|(_, v)| v.read(cx).matches_table_query(&target, cx))
            .map(|(k, v)| (k.clone(), v.clone()))
        {
            cx.emit(LibraryEvent::Open {
                key,
                view: view.clone(),
            });
            cx.defer_in(window, move |_, window, cx| {
                view.update(cx, |note, cx| note.open_table_query(window, cx))
            });
            return;
        }
        self.create_note(Some(target), window, cx);
    }
    fn create_note(
        &mut self,
        target: Option<crate::database::OpenTable>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ready || self.loading || self.opening {
            return;
        }
        if self.documents.len() >= 16 || self.index.entries.len() >= 2000 {
            feedback::error(t(cx, "notes.open_limit"), cx);
            return;
        }
        self.opening = true;
        let query_open = target.is_some();
        let mut note = Note::new("");
        note.cells.push(Cell::new(
            "",
            CellContent::Sql {
                source: String::new(),
                connection_id: None,
                database: None,
            },
        ));
        if let Some(target) = target {
            note.title = target.object.name.clone();
            note.default_connection_id = Some(target.connection);
            note.default_database = Some(target.database.clone());
            note.table_query = Some((target.object.schema.clone(), target.object.name.clone()));
            note.table_query_origin = Some(turbodbn_core::notebook::TableQueryOrigin {
                connection: target.connection,
                database: target.database.clone(),
                schema: target.object.schema.clone(),
                table: target.object.name.clone(),
            });
            if let CellContent::Sql { source, .. } = &mut note.cells[0].content {
                *source = format!(
                    "SELECT * FROM {};",
                    turbodbn_services::database::workbench::qualified(
                        target.kind,
                        &target.database,
                        &target.object
                    )
                );
            }
        }
        let directory = Self::directory(cx);
        let date = chrono::Local::now().format("%y%m%d").to_string();
        let work = cx
            .background_executor()
            .spawn(async move { note_store::create_dated(&directory, &date, note) });
        cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.opening = false;
                    let success = result.is_ok();
                    match result {
                        Ok((path, loaded)) => {
                            this.install(path.clone(), true, loaded, window, cx);
                            if query_open {
                                cx.defer_in(window, move |this, window, cx| {
                                    if let Some(view) = this
                                        .documents
                                        .values()
                                        .find(|v| v.read(cx).path == path)
                                        .cloned()
                                    {
                                        view.update(cx, |note, cx| {
                                            note.open_table_query(window, cx)
                                        });
                                    }
                                });
                            }
                        }
                        Err(e) => feedback::error(e, cx),
                    }
                    cx.emit(LibraryEvent::Settled {
                        key: String::new(),
                        success,
                    });
                    cx.notify();
                })
            });
        })
        .detach();
    }
    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening || !self.ready || self.loading || self.deleting.contains(&path) {
            return;
        }
        if let Some((key, view)) = self
            .documents
            .iter()
            .find(|(_, view)| view.read(cx).path == path)
            .map(|(key, view)| (key.clone(), view.clone()))
        {
            self.update_entry(view.read(cx).entry(cx));
            self.persist();
            cx.emit(LibraryEvent::Open { key, view });
            return;
        }
        let draft = self
            .index
            .entries
            .iter()
            .find(|e| e.path == path)
            .is_some_and(|e| e.draft);
        self.opening = true;
        let source = path.clone();
        let work = cx
            .background_executor()
            .spawn(async move { note_store::load(&source) });
        cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.opening = false;
                    let success = result.is_ok();
                    match result {
                        Ok(loaded) => this.install(path, draft, loaded, window, cx),
                        Err(error) => {
                            this.pending_file_action = None;
                            feedback::error(error, cx);
                        }
                    }
                    cx.emit(LibraryEvent::Settled {
                        key: String::new(),
                        success,
                    });
                    cx.notify();
                })
            });
        })
        .detach();
    }
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t(cx, "notes.open")),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let _ = cx.update(|window, cx| {
                    view.update(cx, |this, cx| this.open_path(path, window, cx))
                });
            }
        })
        .detach();
    }
    pub fn close(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(view) = self.documents.remove(key) {
            self.index.open.retain(|path| *path != view.read(cx).path);
        }
        self.subscriptions.remove(key);
        self.persist();
        cx.notify();
    }
    pub fn pending(&self, cx: &App) -> bool {
        self.opening
            || !self.deleting.is_empty()
            || self.index_revision != self.index_saved
            || self.documents.values().any(|n| n.read(cx).pending())
    }
    pub fn running(&self, cx: &App) -> bool {
        self.documents.values().any(|n| n.read(cx).running(cx))
    }
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        for view in self.documents.values() {
            view.update(cx, |view, cx| view.save_now(cx));
        }
        self.persist();
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        for view in self.documents.values() {
            view.update(cx, |view, cx| view.stop(cx));
        }
    }
    pub fn tools(&self, cx: &Context<Self>) -> AnyElement {
        let mut recent = self.index.entries.clone();
        recent.sort_by_key(|e| std::cmp::Reverse(e.used));
        recent.truncate(5);
        let view = cx.entity();
        div()
            .h_flex()
            .gap_1()
            .child(
                Button::new("new-note")
                    .ghost()
                    .icon(IconName::Plus)
                    .tooltip(t(cx, "notes.new"))
                    .disabled(!self.ready || self.loading || self.opening)
                    .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
            )
            .child(
                Button::new("open-note")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .tooltip(t(cx, "notes.open"))
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
            )
            .child(
                Button::new("recent-notes")
                    .ghost()
                    .icon(IconName::Redo)
                    .tooltip(t(cx, "notes.recent"))
                    .dropdown_menu(move |mut menu, _, cx| {
                        for entry in &recent {
                            let path = entry.path.clone();
                            let view = view.clone();
                            menu = menu.item(
                                PopupMenuItem::element({
                                    let title = entry_title(entry, cx);
                                    move |_, cx| {
                                        div()
                                            .h_flex()
                                            .gap_2()
                                            .child(crate::assets::note_icon(2, cx))
                                            .child(title.clone())
                                    }
                                })
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.open_path(path.clone(), window, cx)
                                    })
                                }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
}
fn entry_title(entry: &Entry, cx: &App) -> String {
    if !entry.title.trim().is_empty() {
        return entry.title.clone();
    }
    let stem = entry.path.file_stem().unwrap_or_default().to_string_lossy();
    if uuid::Uuid::parse_str(&stem).is_ok() {
        t(cx, "notes.legacy_title").to_string()
    } else {
        stem.into_owned()
    }
}
impl Render for NoteLibrary {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.directory != Self::directory(cx) && !self.loading && self.ready {
            self.load(window, cx);
        }
        let filter = self.search.read(cx).value().to_lowercase();
        let mut groups = self.database.read(cx).note_projects();
        for entry in &self.index.entries {
            if !groups.contains(&entry.project) {
                groups.push(entry.project.clone());
            }
        }
        let mut body = div()
            .id("notes-sidebar")
            .v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_2()
            .child(Input::new(&self.search).cleanable(true));
        if self.loading {
            body = body.child(t(cx, "notes.loading"));
        }
        if !self.error.is_empty() {
            body = body
                .child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(self.error.clone()),
                )
                .child(
                    Button::new("reload-notes")
                        .icon(crate::assets::refresh_icon())
                        .label(t(cx, "notes.retry"))
                        .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                );
        }
        let mut list = div().v_flex().gap_1();
        for group in groups {
            let folded = self.collapsed.contains(&group) && filter.is_empty();
            let name = group.clone();
            let entries = self
                .index
                .entries
                .iter()
                .filter(|e| {
                    e.project == group
                        && (filter.is_empty()
                            || format!("{} {}", entry_title(e, cx), e.project)
                                .to_lowercase()
                                .contains(&filter))
                })
                .cloned()
                .collect::<Vec<_>>();
            if entries.is_empty() && !filter.is_empty() {
                continue;
            }
            let target = group.clone();
            let drag = group.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("note-project-row-{group}")))
                    .h_flex()
                    .on_drop(cx.listener(move |this, source: &ProjectDrag, _, cx| {
                        this.database
                            .update(cx, |db, cx| db.move_note_project(&source.0, &target, cx));
                    }))
                    .child(
                        Button::new(SharedString::from(format!("note-project-{group}")))
                            .ghost()
                            .justify_start()
                            .icon(if folded {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .flex_1()
                            .min_w_0()
                            .label(if group.is_empty() {
                                t(cx, "dbwork.ungrouped")
                            } else {
                                group.into()
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&name) {
                                    this.collapsed.insert(name.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when(!drag.is_empty(), |row| {
                        row.child(
                            div()
                                .id(SharedString::from(format!("note-project-grip-{drag}")))
                                .w(px(24.))
                                .h(px(28.))
                                .h_flex()
                                .justify_center()
                                .cursor_grab()
                                .child(gpui_component::Icon::new(IconName::Menu))
                                .on_drag(ProjectDrag(drag), |drag, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| drag.clone())
                                }),
                        )
                    }),
            );
            if !folded {
                for entry in entries {
                    let path = entry.path.clone();
                    let file_path = path.clone();
                    let library = cx.entity().downgrade();
                    list = list.child(
                        Button::new(SharedString::from(format!(
                            "note-file-{}",
                            entry.path.display()
                        )))
                        .ghost()
                        .pl(px(28.))
                        .child(crate::assets::note_icon(2, cx))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(entry_title(&entry, cx)),
                        )
                        .justify_start()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_path(path.clone(), window, cx)
                        }))
                        .context_menu(move |mut menu, _, cx| {
                            for (action, key, icon) in [
                                (
                                    crate::notebook::FileAction::Rename,
                                    "notes.rename",
                                    IconName::ALargeSmall,
                                ),
                                (
                                    crate::notebook::FileAction::SaveAs,
                                    "notes.save_as",
                                    IconName::FolderOpen,
                                ),
                                (
                                    crate::notebook::FileAction::Project,
                                    "notes.project",
                                    IconName::Folder,
                                ),
                                (
                                    crate::notebook::FileAction::Reveal,
                                    "notes.reveal",
                                    IconName::ExternalLink,
                                ),
                            ] {
                                let library = library.clone();
                                let path = file_path.clone();
                                menu =
                                    menu.item(PopupMenuItem::new(t(cx, key)).icon(icon).on_click(
                                        move |_, window, cx| {
                                            let _ = library.update(cx, |this, cx| {
                                                this.file_action(path.clone(), action, window, cx)
                                            });
                                        },
                                    ));
                            }
                            let library = library.clone();
                            let path = file_path.clone();
                            menu.separator().item(
                                PopupMenuItem::new(t(cx, "notes.delete_file"))
                                    .icon(IconName::Delete)
                                    .on_click(move |_, window, cx| {
                                        let _ = library.update(cx, |this, cx| {
                                            this.delete_note(path.clone(), window, cx)
                                        });
                                    }),
                            )
                        }),
                    );
                }
            }
        }
        body.child(
            div()
                .id("note-list-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(list),
        )
    }
}

#[derive(Clone)]
struct ProjectDrag(String);
impl Render for ProjectDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_2().child(self.0.clone())
    }
}

/// A second presentation of the same library, not a second index or document owner.
pub struct NoteHome {
    library: Entity<NoteLibrary>,
    _subscription: Subscription,
}
impl NoteHome {
    pub fn new(library: Entity<NoteLibrary>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&library, |_, _, cx| cx.notify());
        Self {
            library,
            _subscription: subscription,
        }
    }
}
impl Render for NoteHome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.library.update(cx, |library, cx| library.home(cx))
    }
}
impl NoteLibrary {
    fn home(&self, cx: &Context<Self>) -> AnyElement {
        let mut recent = self
            .index
            .entries
            .iter()
            .filter(|e| e.used > 0)
            .cloned()
            .collect::<Vec<_>>();
        recent.sort_by_key(|e| std::cmp::Reverse(e.used));
        recent.truncate(5);
        let disabled = !self.ready || self.loading || self.opening;
        let mut body = div()
            .id("note-home")
            .v_flex()
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .gap_4()
            .child(div().text_lg().child(t(cx, "notes.welcome")))
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(
                        Button::new("home-new-note")
                            .icon(IconName::Plus)
                            .label(t(cx, "notes.new"))
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
                    )
                    .child(
                        Button::new("home-open-note")
                            .icon(IconName::FolderOpen)
                            .label(t(cx, "notes.open"))
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "notes.recent")),
            );
        if recent.is_empty() {
            body = body.child(t(
                cx,
                if self.loading {
                    "notes.loading"
                } else {
                    "notes.recent_empty"
                },
            ));
        }
        for entry in recent {
            let path = entry.path.clone();
            let project = if entry.project.is_empty() {
                t(cx, "dbwork.ungrouped").to_string()
            } else {
                entry.project.clone()
            };
            body =
                body.child(
                    Button::new(SharedString::from(format!(
                        "home-note-{}",
                        entry.path.display()
                    )))
                    .ghost()
                    .justify_start()
                    .max_w(px(720.))
                    .disabled(disabled)
                    .child(crate::assets::note_icon(2, cx))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(entry_title(&entry, cx)),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(project),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_path(path.clone(), window, cx)
                    })),
                );
        }
        if !self.error.is_empty() {
            body = body
                .child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(self.error.clone()),
                )
                .child(
                    Button::new("home-retry")
                        .icon(crate::assets::refresh_icon())
                        .label(t(cx, "notes.retry"))
                        .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                );
        }
        body.into_any_element()
    }
}
