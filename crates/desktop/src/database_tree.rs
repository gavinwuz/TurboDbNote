//! Independent, bounded scopes for the merged navigation tree. No implicit connections.
use super::*;
use crate::locale::{t, tr};
use gpui_component::Sizable;
use std::collections::{HashMap, HashSet};

const MAX_OBJECTS: usize = 20_000;
#[path = "account_admin_ui.rs"]
pub(crate) mod account_admin_ui;
#[path = "database_tree_accounts.rs"]
mod accounts;
#[path = "database_tree_controls.rs"]
mod controls;
#[path = "database_tree_search.rs"]
mod search;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Scope {
    id: Uuid,
    database: String,
    generation: u64,
}
impl Scope {
    fn key(&self, suffix: &str) -> String {
        // Length-delimited JSON avoids collisions between identifiers containing separators.
        serde_json::to_string(&(self.id, &self.database, suffix)).unwrap()
    }
}
fn page_key(scope: &Scope, category: &str) -> (Scope, String) {
    (
        scope.clone(),
        crate::object_browser::bucket(category).to_owned(),
    )
}
#[derive(Default)]
struct Page {
    objects: Vec<Object>,
    columns: HashMap<String, Vec<workbench::Column>>,
    query: String,
    busy: HashSet<String>,
    error: Option<String>,
    loaded: bool,
    more: bool,
    offset: usize,
    updated: i64,
}
#[derive(Default)]
struct DatabaseNames {
    generation: u64,
    sequence: u64,
    rule: crate::catalog_cache::DatabaseRule,
    query: String,
    names: Vec<String>,
    more: bool,
    loaded: bool,
    busy: bool,
    error: Option<String>,
}
struct DatabaseFilter {
    input: Entity<InputState>,
    open: bool,
    _subscription: Subscription,
}
#[derive(Default)]
pub(super) struct Tree {
    database_filters: HashMap<Uuid, DatabaseFilter>,
    databases: HashMap<Uuid, DatabaseNames>,
    search_scope: controls::SearchScope,
    filter: search::Filter,
    pub(super) catalog_revision: u64,
    pages: HashMap<(Scope, String), Page>,
    scroll: UniformListScrollHandle,
    sticky_offset: Pixels,
    scroll_layout: Option<(bool, Option<Uuid>)>,
    layout_scrolls: HashMap<(bool, Option<Uuid>), UniformListScrollHandle>,
    sticky_markers: Vec<(usize, bool)>,
    revision: u64,
    fingerprint: Option<u64>,
    rows: std::rc::Rc<Vec<Row>>,
}
#[derive(Clone)]
enum Row {
    Group(String),
    Connection(std::rc::Rc<Profile>),
    Database(Scope),
    Databases(Scope),
    Users(Scope),
    User(Scope, Object),
    UsersPage(Scope),
    DatabaseFilter(Uuid),
    DatabaseMore(Uuid),
    Category(Scope, String),
    Object(Scope, String, Object),
    Column(Scope, workbench::Column),
    Page(Scope, String),
    FilterMore(Uuid),
    FilterUnindexed(Uuid),
    Notice(&'static str),
    LayoutSettings(Scope),
}
/// Markers include project boundaries so an old connection never sticks over another project.
fn sticky_header(markers: &[(usize, bool)], offset: f32) -> Option<(usize, f32)> {
    let marker = markers
        .partition_point(|(index, _)| (*index as f32) * 32. < offset)
        .checked_sub(1)?;
    let (index, connection) = markers[marker];
    if !connection {
        return None;
    }
    let top = markers
        .get(marker + 1)
        .map_or(0., |(next, _)| (*next as f32 * 32. - offset - 32.).min(0.));
    Some((index, top))
}
impl DatabaseView {
    pub(super) fn tree_cached_note_columns(
        &self,
        id: Uuid,
        database: &str,
        object: &Object,
    ) -> Option<Vec<workbench::Column>> {
        let scope = Scope {
            id,
            database: database.into(),
            generation: self.note_generation(id),
        };
        self.tree
            .pages
            .get(&page_key(&scope, "table"))
            .and_then(|page| page.columns.get(&Self::object_key(object)))
            .cloned()
    }
    fn tree_cache_log(
        &self,
        task: Uuid,
        id: Uuid,
        started: std::time::Instant,
        error: Option<&str>,
        finished: bool,
        cx: &mut Context<Self>,
    ) {
        use crate::operation_log::{Kind, Phase, Record};
        let phase = if !finished {
            Phase::Started
        } else if error == Some("mcp.stale") {
            Phase::Cancelled
        } else if error.is_some() {
            Phase::Failed
        } else {
            Phase::Succeeded
        };
        let mut record = Record::new(task, Kind::Metadata, phase);
        record.source = Some(id);
        record.elapsed_ms = started.elapsed().as_millis() as u64;
        record.context = t(cx, "database.cache.operation").to_string();
        if let Some(error) = error {
            record.failure = crate::operation_log::failure_class(error);
        }
        if let Some(output) = &self.output {
            output.update(cx, |output, cx| output.operation(record, cx));
        }
    }
    pub(super) fn database_filter_active(&self, id: Uuid, cx: &App) -> bool {
        self.tree
            .database_filters
            .get(&id)
            .is_some_and(|filter| filter.open || !filter.input.read(cx).value().is_empty())
    }
    fn ensure_database_filter(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.tree.database_filters.entry(id).or_insert_with(|| {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(t(cx, "database.tree.database_filter"))
            });
            let subscription = cx.subscribe(&input, |_, _, event, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    cx.notify();
                }
            });
            DatabaseFilter {
                input,
                open: false,
                _subscription: subscription,
            }
        });
    }
    pub(super) fn toggle_database_filter(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.note_connection_state(id) != Some(true) {
            return;
        }
        let branch = Scope {
            id,
            database: self
                .note_profile(id)
                .map(|profile| profile.database.clone())
                .unwrap_or_default(),
            generation: self.note_generation(id),
        };
        self.ensure_database_filter(id, window, cx);
        let filter = self.tree.database_filters.get_mut(&id).unwrap();
        filter.open = !filter.open || !self.navigation.tree_connections.contains(&id);
        if filter.open {
            self.navigation
                .tree_nodes
                .remove(&branch.key("databases-closed"));
            self.navigation.tree_connections.insert(id);
            filter.input.update(cx, |input, cx| input.focus(window, cx));
            if let Some(index) = self
                .tree
                .rows
                .iter()
                .position(|row| matches!(row, Row::Databases(scope) if scope.id == id))
            {
                let handle = self.tree.scroll.0.borrow().base_handle.clone();
                if px((index + 2) as f32 * 32.) + handle.offset().y > handle.bounds().size.height {
                    self.tree
                        .scroll
                        .scroll_to_item(index + 1, ScrollStrategy::Bottom);
                }
            }
            self.persist_navigation();
        }
        cx.notify();
    }
    pub(super) fn tree_sync_databases(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(profile) = self.note_profile(id).cloned() else {
            return;
        };
        let Some(info) = self.info.get(&id) else {
            return;
        };
        if info.database_list_error.is_some() {
            self.tree_database_page(id, false, cx);
            return;
        }
        let names = self.directory_databases(id);
        let cache = crate::catalog_cache::Cache::new(&self.file, &profile, "", "databases");
        let generation = self.note_generation(id);
        let rule = self.tree_database_rule(id);
        let state = self.tree.databases.entry(id).or_default();
        state.busy = true;
        if state.generation != generation {
            state.loaded = false;
            state.names.clear();
        }
        state.generation = generation;
        state.error = None;
        state.sequence += 1;
        let sequence = state.sequence;
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.tree_cache_log(task, id, started, None, false, cx);
        let snapshot = crate::catalog_cache::Cache::ticket();
        cx.spawn(async move |view, cx| {
            let worker_rule = rule.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    cache.begin(&snapshot)?;
                    for chunk in names.chunks(200) {
                        let objects = chunk
                            .iter()
                            .map(|name| Object {
                                name: name.clone(),
                                kind: "database".into(),
                                schema: String::new(),
                                parent: String::new(),
                                comment: String::new(),
                            })
                            .collect::<Vec<_>>();
                        cache.append(&snapshot, &objects)?;
                    }
                    cache.commit(&snapshot)?;
                    cache.read_filtered("", 0, 200, &worker_rule)
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.tree_cache_log(
                    task,
                    id,
                    started,
                    result.as_ref().err().map(String::as_str),
                    true,
                    cx,
                );
                if this.note_generation(id) != generation
                    || this
                        .tree
                        .databases
                        .get(&id)
                        .is_none_or(|state| state.sequence != sequence)
                {
                    return;
                }
                let state = this.tree.databases.entry(id).or_default();
                state.busy = false;
                match result {
                    Ok(Some(page)) => {
                        state.names = page.objects.into_iter().map(|o| o.name).collect();
                        state.query.clear();
                        state.more = page.more;
                        state.loaded = true;
                        state.rule = rule;
                        this.tree.catalog_revision += 1;
                    }
                    Ok(None) => state.error = Some("database.cache.storage_error".into()),
                    Err(error) => {
                        state.error = Some(error.clone());
                        feedback::error(crate::locale::message(cx, &error), cx);
                    }
                }
                this.tree.revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
    fn tree_database_page(&mut self, id: Uuid, more: bool, cx: &mut Context<Self>) {
        let Some(profile) = self.note_profile(id) else {
            return;
        };
        let cache = crate::catalog_cache::Cache::new(&self.file, profile, "", "databases");
        let query = self
            .tree
            .database_filters
            .get(&id)
            .map(|f| f.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let generation = self.note_generation(id);
        let rule = self.tree_database_rule(id);
        let state = self.tree.databases.entry(id).or_default();
        if state.busy {
            return;
        }
        let append = more && state.query == query && state.rule == rule;
        let offset = if append { state.names.len() } else { 0 };
        state.busy = true;
        if state.generation != generation {
            state.loaded = false;
            state.names.clear();
        }
        state.generation = generation;
        state.sequence += 1;
        let sequence = state.sequence;
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.tree_cache_log(task, id, started, None, false, cx);
        cx.spawn(async move |view, cx| {
            let needle = query.clone();
            let worker_rule = rule.clone();
            let result = cx
                .background_executor()
                .spawn(async move { cache.read_filtered(&needle, offset, 200, &worker_rule) })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.tree_cache_log(
                    task,
                    id,
                    started,
                    result.as_ref().err().map(String::as_str),
                    true,
                    cx,
                );
                if this.note_generation(id) != generation
                    || this
                        .tree
                        .databases
                        .get(&id)
                        .is_none_or(|state| state.sequence != sequence)
                {
                    return;
                }
                let state = this.tree.databases.entry(id).or_default();
                state.busy = false;
                match result {
                    Ok(Some(page)) => {
                        if !append {
                            state.names.clear();
                        }
                        state.names.extend(page.objects.into_iter().map(|o| o.name));
                        state.more = page.more;
                        state.query = query;
                        state.loaded = true;
                        state.rule = rule;
                        state.error = None;
                    }
                    Ok(None) => state.error = Some("database.cache.unindexed".into()),
                    Err(error) => state.error = Some(error),
                }
                this.tree.revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
    fn tree_scope_current(&self, scope: &Scope) -> bool {
        self.note_generation(scope.id) == scope.generation
            && self.note_connection_state(scope.id) == Some(true)
    }
    fn tree_select(&mut self, scope: &Scope, cx: &mut Context<Self>) -> bool {
        if !self.tree_scope_current(scope) {
            return false;
        }
        if self.selected_database.as_ref() == Some(&(scope.id, scope.database.clone())) {
            return true;
        }
        if self
            .active_operation
            .as_ref()
            .is_some_and(|(_, _, kind)| !objects_ui::is_metadata_read(kind))
            || self.workbench_target.is_some()
        {
            feedback::error(t(cx, "dbwork.switch.busy"), cx);
            return false;
        }
        self.retire_browser(cx);
        self.clear_object_view();
        self.selected_database = Some((scope.id, scope.database.clone()));
        self.navigation
            .databases
            .insert(scope.id, scope.database.clone());
        self.persist_navigation();
        cx.notify();
        true
    }
    fn tree_toggle(&mut self, key: String, cx: &mut Context<Self>) -> bool {
        if !self.navigation.tree_nodes.contains(&key) && self.navigation.tree_nodes.len() >= 1024 {
            feedback::error(t(cx, "database.tree.limit"), cx);
            return false;
        }
        let open = if self.navigation.tree_nodes.remove(&key) {
            false
        } else {
            self.navigation.tree_nodes.insert(key.clone());
            if serde_json::to_vec(&self.navigation).is_ok_and(|bytes| bytes.len() > 60 * 1024) {
                self.navigation.tree_nodes.remove(&key);
                feedback::error(t(cx, "database.tree.limit"), cx);
                return false;
            }
            true
        };
        self.persist_navigation();
        cx.notify();
        open
    }
    fn tree_refresh_database(&mut self, scope: Scope, cx: &mut Context<Self>) {
        let mut categories = self
            .tree
            .pages
            .keys()
            .filter(|(s, _)| s == &scope)
            .map(|(_, category)| category.clone())
            .collect::<HashSet<_>>();
        categories.insert("relations".into());
        for category in categories {
            self.tree_read(
                scope.clone(),
                if category == "relations" {
                    "table".into()
                } else {
                    category
                },
                false,
                true,
                cx,
            );
        }
    }
    pub(crate) fn load_note_catalog(&mut self, id: Uuid, database: String, cx: &mut Context<Self>) {
        if !self.note_database_visible(id, &database) {
            return;
        }
        self.tree_read(
            Scope {
                id,
                database,
                generation: self.note_generation(id),
            },
            "table".into(),
            false,
            true,
            cx,
        );
    }
    fn tree_load(&mut self, scope: Scope, category: String, more: bool, cx: &mut Context<Self>) {
        self.tree_read(scope, category, more, false, cx);
    }
    fn tree_read(
        &mut self,
        scope: Scope,
        category: String,
        more: bool,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.tree_scope_current(&scope) {
            return;
        }
        let Some(profile) = self.note_profile(scope.id) else {
            return;
        };
        let bucket = crate::object_browser::bucket(&category).to_owned();
        let cache = crate::catalog_cache::Cache::new(&self.file, profile, &scope.database, &bucket);
        let query = String::new();
        let key = page_key(&scope, &category);
        let page = self.tree.pages.entry(key.clone()).or_default();
        if !page.busy.is_empty() {
            return;
        }
        let append = more && page.loaded && page.query == query;
        let offset = if append { page.offset } else { 0 };
        page.query = query.clone();
        page.busy.insert("catalog".into());
        page.error = None;
        self.tree.revision += 1;
        let paged = self.paged_catalog_supported(scope.id)
            || self
                .note_profile(scope.id)
                .is_some_and(|p| p.kind == DatabaseKind::MySql);
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.tree_cache_log(task, scope.id, started, None, false, cx);
        let snapshot = crate::catalog_cache::Cache::ticket();
        cx.spawn(async move |view, cx| {
            let worker_cache = cache.clone();
            let worker_query = query.clone();
            let mut result = cx
                .background_executor()
                .spawn(async move {
                    if refresh {
                        Ok(None)
                    } else {
                        worker_cache.read(&worker_query, offset, 200)
                    }
                })
                .await;
            let synchronized = matches!(result, Ok(None));
            if synchronized {
                let mut remote_offset = 0;
                let sync = async {
                    let worker = cache.clone();
                    let token = snapshot.clone();
                    cx.background_executor()
                        .spawn(async move { worker.begin(&token) })
                        .await?;
                    loop {
                        let receiver = view
                            .update(cx, |this, cx| {
                                if !this.tree_scope_current(&scope) {
                                    return None;
                                }
                                Some(this.mcp_request(
                                    scope.id,
                                    scope.generation,
                                    scope.database.clone(),
                                    if paged {
                                        Action::BrowseCatalog {
                                            category: bucket.clone(),
                                            search: String::new(),
                                            offset: remote_offset,
                                            limit: 200,
                                            include_comments: false,
                                        }
                                    } else {
                                        Action::Catalog
                                    },
                                    cx,
                                ))
                            })
                            .map_err(|_| "mcp.stale".to_owned())?
                            .ok_or("mcp.stale")?;
                        let reply = receiver.recv().await.map_err(|_| "mcp.stale")??;
                        let (objects, more) = match reply {
                            Reply::CatalogPage { objects, more } => (objects, more),
                            Reply::Catalog { objects, notice }
                                if notice.is_empty() || notice.starts_with("MySQL 4.") =>
                            {
                                (objects, false)
                            }
                            Reply::Catalog { notice, .. } => return Err(notice),
                            _ => return Err("database.tree.unsupported".into()),
                        };
                        if more && objects.is_empty() {
                            return Err("browser.response_failed".into());
                        }
                        remote_offset += objects.len();
                        let worker = cache.clone();
                        let token = snapshot.clone();
                        cx.background_executor()
                            .spawn(async move { worker.append(&token, &objects) })
                            .await?;
                        if !more {
                            break;
                        }
                    }
                    if !view
                        .read_with(cx, |this, _| this.tree_scope_current(&scope))
                        .unwrap_or(false)
                    {
                        return Err("mcp.stale".into());
                    }
                    let worker = cache.clone();
                    let token = snapshot.clone();
                    cx.background_executor()
                        .spawn(async move { worker.commit(&token) })
                        .await
                }
                .await;
                result = match sync {
                    Ok(()) => {
                        let worker = cache.clone();
                        let needle = query.clone();
                        cx.background_executor()
                            .spawn(async move { worker.read(&needle, offset, 200) })
                            .await
                    }
                    Err(error) => {
                        let worker = cache.clone();
                        let cleanup = cx
                            .background_executor()
                            .spawn(async move { worker.discard(&snapshot) })
                            .await;
                        Err(cleanup.err().unwrap_or(error))
                    }
                };
            }
            let _ = view.update(cx, |this, cx| {
                this.tree_cache_log(
                    task,
                    scope.id,
                    started,
                    result.as_ref().err().map(String::as_str),
                    true,
                    cx,
                );
                if !this.tree_scope_current(&scope) {
                    this.tree.pages.remove(&key);
                    return;
                }
                let Some(page) = this.tree.pages.get_mut(&key) else {
                    return;
                };
                page.busy.remove("catalog");
                let success = result.as_ref().is_ok_and(|v| v.is_some());
                match result {
                    Ok(Some(local)) => {
                        if !append {
                            page.objects.clear();
                            page.columns.clear();
                        }
                        page.offset = offset + local.objects.len();
                        page.objects.extend(local.objects);
                        page.loaded = true;
                        page.more = local.more;
                        page.updated = local.updated;
                        if synchronized {
                            this.tree.catalog_revision += 1;
                        }
                    }
                    Ok(None) => page.error = Some("database.cache.storage_error".into()),
                    Err(error) => page.error = Some(error),
                }
                cx.emit(NoteCatalogReady(
                    scope.id,
                    scope.database.clone(),
                    scope.generation,
                    success,
                ));
                this.tree.revision += 1;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn tree_columns(
        &mut self,
        scope: Scope,
        category: String,
        object: Object,
        cx: &mut Context<Self>,
    ) {
        let key = page_key(&scope, &category);
        let object_key = Self::object_key(&object);
        let page = self.tree.pages.entry(key.clone()).or_default();
        if page.columns.contains_key(&object_key) || !page.busy.insert(object_key.clone()) {
            return;
        }
        self.tree.revision += 1;
        let paged = self.paged_catalog_supported(scope.id);
        let action = if paged {
            Action::BrowseColumns(object)
        } else {
            Action::Describe(object)
        };
        let rx = self.mcp_request(
            scope.id,
            scope.generation,
            scope.database.clone(),
            action,
            cx,
        );
        cx.spawn(async move |view, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update(cx, |this, cx| {
                if !this.tree_scope_current(&scope) {
                    this.tree.pages.remove(&key);
                    return;
                }
                let count: usize = this
                    .tree
                    .pages
                    .values()
                    .flat_map(|p| p.columns.values())
                    .map(Vec::len)
                    .sum();
                let column_error = result.as_ref().err().cloned();
                if let Some(page) = this.tree.pages.get_mut(&key) {
                    page.busy.remove(&object_key);
                    match result {
                        Ok(Reply::Schema(schema))
                            if count + schema.columns.len() <= MAX_OBJECTS =>
                        {
                            page.columns.insert(object_key, schema.columns);
                        }
                        Ok(Reply::Schema(_)) => page.error = Some("database.tree.limit".into()),
                        Err(error) => page.error = Some(error),
                        _ => page.error = Some("database.tree.unsupported".into()),
                    }
                }
                if !this.tree_object_query(cx).is_empty()
                    && let Some(error) = column_error
                {
                    feedback::error(crate::locale::message(cx, &error), cx);
                }
                this.tree.revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn table_card_action(
        &mut self,
        target: OpenTable,
        mode: Option<crate::table_view::Mode>,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tree_action(
            Scope {
                id: target.connection,
                database: target.database,
                generation: target.generation,
            },
            target.object,
            mode,
            action,
            window,
            cx,
        );
    }
    fn tree_action(
        &mut self,
        scope: Scope,
        object: Object,
        mode: Option<crate::table_view::Mode>,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tree_scope_current(&scope) {
            feedback::error(t(cx, "mcp.stale"), cx);
            return;
        }
        if let Some(mode) = mode {
            let Some(p) = self.note_profile(scope.id) else {
                return;
            };
            cx.emit(OpenTable {
                connection: scope.id,
                database: scope.database,
                object,
                mode,
                generation: scope.generation,
                kind: p.kind,
                writable: p.supports_mutation()
                    || (mode == crate::table_view::Mode::Grid && p.supports_grid_mutation()),
                mysql_raw_gbk: p.mysql_raw_gbk,
            });
        } else if action == "structure" {
            feedback::ddl_context(
                feedback::DdlContext {
                    note: None,
                    candidates: vec![],
                    origin: None,
                    scope: Some((scope.id, scope.database)),
                    object: Some(object),
                    open: true,
                },
                cx,
            );
        } else if action == "name" {
            cx.write_to_clipboard(ClipboardItem::new_string(object.name));
            feedback::success(t(cx, "feedback.copied"), cx);
        } else {
            // Existing edit/delete/DDL flows retain their previews and confirmations.
            if !self.follow_connection(scope.id, false, cx) {
                return;
            }
            if self.selected_database.as_ref() != Some(&(scope.id, scope.database.clone())) {
                self.select_database(scope.id, scope.database.clone(), cx);
            }
            if self.selected_database.as_ref() == Some(&(scope.id, scope.database)) {
                self.object_action(object, action, window, cx);
            }
        }
    }
    pub(super) fn render_merged_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_database_navigation_tree(false, window, cx)
    }
    pub(super) fn render_database_navigation_tree(
        &mut self,
        split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_database.as_ref().map(|(id, _)| *id);
        if split && let Some(id) = selected {
            self.ensure_database_filter(id, window, cx);
        }

        let layout = (split, if split { selected } else { None });
        self.tree.layout_scrolls.retain(|(_, id), _| {
            id.is_none_or(|id| self.profiles.iter().any(|profile| profile.id == id))
        });
        if self.tree.scroll_layout != Some(layout) {
            if let Some(previous) = self.tree.scroll_layout {
                self.tree
                    .layout_scrolls
                    .insert(previous, self.tree.scroll.clone());
            }
            self.tree.scroll = self
                .tree
                .layout_scrolls
                .get(&layout)
                .cloned()
                .unwrap_or_default();
            self.tree.scroll_layout = Some(layout);
        }
        self.tree_filter_update(cx);
        let filtering = !self.tree_object_query(cx).is_empty();
        let valid: HashSet<_> = self
            .profiles
            .iter()
            .filter(|p| self.note_connection_state(p.id) == Some(true))
            .map(|p| (p.id, self.note_generation(p.id)))
            .collect();
        self.tree
            .pages
            .retain(|(s, _), _| valid.contains(&(s.id, s.generation)));
        self.tree
            .database_filters
            .retain(|id, _| self.profiles.iter().any(|profile| profile.id == *id));
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        split.hash(&mut hasher);
        if split {
            selected.hash(&mut hasher);
        }
        self.tree.revision.hash(&mut hasher);
        self.tree.search_scope.hash(&mut hasher);
        self.search.read(cx).value().hash(&mut hasher);
        self.tree_filter.read(cx).value().hash(&mut hasher);
        serde_json::to_string(&self.navigation)
            .unwrap_or_default()
            .hash(&mut hasher);
        for p in &self.profiles {
            (
                p.id,
                p.port,
                p.kind,
                &p.sqlite_path,
                &p.driver_id,
                &p.name,
                &p.group,
                &p.environment,
                &p.host,
                &p.database,
                self.note_generation(p.id),
                self.note_connection_state(p.id),
            )
                .hash(&mut hasher);
            if let Some(filter) = self.tree.database_filters.get(&p.id) {
                filter.open.hash(&mut hasher);
                filter.input.read(cx).value().hash(&mut hasher);
            }
            if let Some(info) = self.info.get(&p.id) {
                info.databases.hash(&mut hasher);
                info.version.hash(&mut hasher);
                info.database_list_error.hash(&mut hasher);
            }
        }
        let fingerprint = hasher.finish();
        let rows = if self.tree.fingerprint == Some(fingerprint) {
            self.tree.rows.clone()
        } else if filtering {
            let rows = self.tree_filter_rows(cx);
            let rows = if split {
                std::rc::Rc::new(
                    rows.iter()
                        .filter(|row| match row {
                            Row::Database(s)
                            | Row::Category(s, _)
                            | Row::Object(s, _, _)
                            | Row::Column(s, _)
                            | Row::Page(s, _)
                            | Row::LayoutSettings(s) => Some(s.id) == selected,
                            Row::Notice(_) => true,
                            _ => false,
                        })
                        .cloned()
                        .collect(),
                )
            } else {
                rows
            };
            self.tree.fingerprint = Some(fingerprint);
            self.tree.sticky_markers = rows
                .iter()
                .enumerate()
                .filter_map(|(index, row)| match row {
                    Row::Connection(_) if !split => Some((index, true)),
                    Row::Database(_) if split => Some((index, true)),
                    Row::Users(_) if split => Some((index, false)),
                    Row::Group(_) => Some((index, false)),
                    _ => None,
                })
                .collect();
            self.tree.rows = rows.clone();
            rows
        } else {
            let search = self.tree_connection_query(cx);
            let mut rows = vec![];
            for group in self.ordered_groups() {
                if !split {
                    rows.push(Row::Group(group.clone()));
                }
                let group_key = if self.navigation.mode == navigation::GroupMode::Environment {
                    format!("environment:{group}")
                } else {
                    group.clone()
                };
                if !split && search.is_empty() && self.navigation.collapsed.contains(&group_key) {
                    continue;
                }
                for p in self.profiles.iter().filter(|p| self.group_key(p) == group) {
                    if (split && selected != Some(p.id)) || !self.connection_matches(p, &search) {
                        continue;
                    }
                    if !split {
                        rows.push(Row::Connection(std::rc::Rc::new(p.clone())));
                    }
                    if !split && !self.navigation.tree_connections.contains(&p.id) {
                        continue;
                    }
                    if self.note_connection_state(p.id) != Some(true) {
                        rows.push(Row::Notice(if self.note_connection_state(p.id).is_none() {
                            "database.branch.connecting"
                        } else {
                            "dbwork.switch.disconnected"
                        }));
                        continue;
                    }
                    let branch = Scope {
                        id: p.id,
                        database: p.database.clone(),
                        generation: self.note_generation(p.id),
                    };
                    rows.push(Row::Databases(branch.clone()));
                    if !self
                        .navigation
                        .tree_nodes
                        .contains(&branch.key("databases-closed"))
                    {
                        let rule = self.tree_database_rule(p.id);
                        let mut names = self.directory_databases(p.id);
                        if names.is_empty() && p.is_file_database() {
                            names.push(
                                PathBuf::from(&p.sqlite_path)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            );
                        }
                        if p.kind == DatabaseKind::PostgreSql {
                            names.retain(|n| n == &p.database);
                        }
                        let filter = self.tree.database_filters.get(&p.id);
                        let database_query = filter
                            .map(|filter| filter.input.read(cx).value().trim().to_lowercase())
                            .unwrap_or_default();
                        if !split
                            && filter.is_some_and(|filter| {
                                filter.open || !filter.input.read(cx).value().is_empty()
                            })
                        {
                            rows.push(Row::DatabaseFilter(p.id));
                        }
                        if let Some(state) = self.tree.databases.get(&p.id).filter(|state| {
                            state.loaded
                                && state.generation == self.note_generation(p.id)
                                && state.rule == rule
                                && state.query == database_query
                        }) {
                            names = state.names.clone();
                        }
                        names.retain(|name| {
                            rule.matches(name)
                                && (database_query.is_empty()
                                    || name.to_lowercase().contains(&database_query))
                        });
                        if names.is_empty() {
                            rows.push(Row::Notice(if database_query.is_empty() {
                                "database.rule.empty"
                            } else {
                                "database.tree.database_empty"
                            }));
                        }
                        for database in names {
                            let scope = Scope {
                                id: p.id,
                                database,
                                generation: self.note_generation(p.id),
                            };
                            rows.push(Row::Database(scope.clone()));
                            if !self.navigation.tree_nodes.contains(&scope.key("database")) {
                                continue;
                            }
                            let version = self
                                .info
                                .get(&p.id)
                                .map(|i| i.version.as_str())
                                .unwrap_or_default();
                            let categories = workbench::categories(p.kind, version);
                            if categories.is_empty() {
                                rows.push(Row::Notice("database.tree.unsupported"));
                                rows.push(Row::LayoutSettings(scope.clone()));
                            }
                            for category in categories {
                                let category = category.to_string();
                                rows.push(Row::Category(scope.clone(), category.clone()));
                                if !self.navigation.tree_nodes.contains(&scope.key(&category)) {
                                    continue;
                                }
                                if let Some(page) =
                                    self.tree.pages.get(&page_key(&scope, &category))
                                {
                                    for object in page.objects.iter().filter(|o| o.kind == category)
                                    {
                                        rows.push(Row::Object(
                                            scope.clone(),
                                            category.clone(),
                                            object.clone(),
                                        ));
                                        let object_key = Self::object_key(object);
                                        if self
                                            .navigation
                                            .tree_nodes
                                            .contains(&scope.key(&object_key))
                                        {
                                            if let Some(columns) = page.columns.get(&object_key) {
                                                rows.extend(columns.iter().cloned().map(
                                                    |column| Row::Column(scope.clone(), column),
                                                ));
                                            } else if page.busy.contains(&object_key) {
                                                rows.push(Row::Notice("ddl.loading"));
                                            }
                                        }
                                    }
                                }
                                if self
                                    .tree
                                    .pages
                                    .get(&page_key(&scope, &category))
                                    .is_some_and(|p| {
                                        p.error.as_deref() == Some("database.tree.unsupported")
                                    })
                                {
                                    rows.push(Row::Notice("database.tree.unsupported"));
                                    rows.push(Row::LayoutSettings(scope.clone()));
                                } else if self
                                    .tree
                                    .pages
                                    .get(&page_key(&scope, &category))
                                    .is_none_or(|page| {
                                        !page.busy.is_empty() || page.error.is_some() || page.more
                                    })
                                {
                                    rows.push(Row::Page(scope.clone(), category));
                                }
                            }
                        }
                        if self
                            .tree
                            .databases
                            .get(&p.id)
                            .is_some_and(|state| state.more || state.busy || state.error.is_some())
                        {
                            rows.push(Row::DatabaseMore(p.id));
                        }
                    }
                    if accounts::supported(p) {
                        rows.push(Row::Users(branch.clone()));
                        if self.navigation.tree_nodes.contains(&branch.key("users"))
                            && let Some(page) = self.tree.pages.get(&page_key(&branch, "user"))
                        {
                            rows.extend(
                                page.objects
                                    .iter()
                                    .cloned()
                                    .map(|object| Row::User(branch.clone(), object)),
                            );
                            if page.more || !page.busy.is_empty() || page.error.is_some() {
                                rows.push(Row::UsersPage(branch.clone()));
                            } else if page.loaded && page.objects.is_empty() {
                                rows.push(Row::Notice("database.users.empty"));
                            }
                        }
                    }
                }
            }
            if split && selected.is_none() {
                rows.push(Row::Notice("notes.choose_connection"));
            }
            let rows = std::rc::Rc::new(rows);
            self.tree.fingerprint = Some(fingerprint);
            self.tree.sticky_markers = rows
                .iter()
                .enumerate()
                .filter_map(|(index, row)| match row {
                    Row::Connection(_) if !split => Some((index, true)),
                    Row::Database(_) if split => Some((index, true)),
                    Row::Users(_) if split => Some((index, false)),
                    Row::Group(_) => Some((index, false)),
                    _ => None,
                })
                .collect();
            self.tree.rows = rows.clone();
            rows
        };
        if !filtering
            && let Some(id) = self.profiles.iter().find_map(|p| {
                if (split && selected != Some(p.id))
                    || (!split && !self.navigation.tree_connections.contains(&p.id))
                    || self.note_connection_state(p.id) != Some(true)
                {
                    return None;
                }
                let state = self.tree.databases.get(&p.id)?;
                let query = self
                    .tree
                    .database_filters
                    .get(&p.id)
                    .map(|f| f.input.read(cx).value().trim().to_lowercase())
                    .unwrap_or_default();
                (!state.busy
                    && state.error.is_none()
                    && (!state.loaded
                        || state.query != query
                        || state.rule != self.tree_database_rule(p.id)))
                .then_some(p.id)
            })
        {
            let owner = cx.entity();
            cx.defer(move |cx| owner.update(cx, |this, cx| this.tree_database_page(id, false, cx)));
        }
        if !filtering
            && let Some(scope) = rows.iter().find_map(|row| {
                if let Row::Users(scope) = row {
                    (self.navigation.tree_nodes.contains(&scope.key("users"))
                        && !self.tree.pages.contains_key(&page_key(scope, "user")))
                    .then(|| scope.clone())
                } else {
                    None
                }
            })
        {
            let owner = cx.entity();
            cx.defer(move |cx| owner.update(cx, |this, cx| this.tree_users_load(scope, false, cx)));
        }
        if !filtering
            && !self
                .tree
                .pages
                .values()
                .any(|page| page.busy.contains("catalog"))
        {
            let next = rows.iter().find_map(|row| {
                let Row::Category(scope, category) = row else {
                    return None;
                };
                if category != "table"
                    && category != "view"
                    && !self.navigation.tree_nodes.contains(&scope.key(category))
                {
                    return None;
                }
                let needs = self
                    .tree
                    .pages
                    .get(&page_key(scope, category))
                    .is_none_or(|page| page.error.is_none() && !page.loaded);
                needs.then(|| (scope.clone(), category.clone()))
            });
            if let Some((scope, category)) = next {
                let owner = cx.entity();
                cx.defer(move |cx| {
                    owner.update(cx, |this, cx| this.tree_load(scope, category, false, cx))
                });
            }
        }
        let offset = f32::from(-self.tree.scroll.0.borrow().base_handle.offset().y).max(0.);
        let sticky = sticky_header(&self.tree.sticky_markers, offset).and_then(|(index, top)| {
            match &rows[index] {
                Row::Connection(_) | Row::Database(_) => Some((index, rows[index].clone(), top)),
                _ => None,
            }
        });
        div()
            .size_full()
            .min_h_0()
            .v_flex()
            .p_2()
            .gap_2()
            .when(!split, |body| {
                body.child(
                    Button::new("tree-new-connection")
                        .icon(IconName::Plus)
                        .label(t(cx, "dbwork.connection.new"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.dirty(cx) {
                                cx.emit(OpenDatabase);
                                feedback::error(t(cx, "database.tree.unsaved"), cx);
                                return;
                            }
                            this.typed_drafts.clear();
                            this.fill(Self::empty_profile(DatabaseKind::MySql), window, cx);
                            this.choosing_type = true;
                            cx.emit(OpenDatabase);
                        })),
                )
            })
            .child(if split {
                selected
                    .map(|id| self.tree_row(Row::DatabaseFilter(id), cx))
                    .unwrap_or_else(|| {
                        div()
                            .child(t(cx, "notes.choose_connection"))
                            .into_any_element()
                    })
            } else {
                self.tree_search_control(cx)
            })
            .when(
                filtering && self.tree.filter.phase != search::Phase::Ready,
                |body| body.child(self.tree_filter_status(cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .min_w_0()
                    .pr(px(12.))
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "merged-database-tree",
                            rows.len(),
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                let offset = this.tree.scroll.0.borrow().base_handle.offset().y;
                                if this.tree.sticky_offset != offset {
                                    this.tree.sticky_offset = offset;
                                    cx.notify();
                                }
                                range
                                    .map(|i| {
                                        div()
                                            .h(px(32.))
                                            .w_full()
                                            .child(this.tree_row(rows[i].clone(), cx))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.tree.scroll.clone())
                        .size_full()
                        .min_h_0(),
                    )
                    .when_some(sticky, |body, (index, row, top)| {
                        if let Row::Database(scope) = row {
                            let owner = cx.entity().downgrade();
                            let menu_scope = scope.clone();
                            let toggle_scope = scope.clone();
                            return body.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right(px(12.))
                                    .top(px(top))
                                    .h(px(32.))
                                    .overflow_hidden()
                                    .bg(cx.theme().background)
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .child(crate::context_menu::scoped(
                                        SharedString::from(format!(
                                            "sticky-db-{}-{}",
                                            scope.id, scope.database
                                        )),
                                        div()
                                            .h_flex()
                                            .size_full()
                                            .child(
                                                Button::new(("sticky-db-toggle", index))
                                                    .ghost()
                                                    .icon(IconName::ChevronDown)
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.tree_toggle_branch(
                                                                toggle_scope.key("database"),
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new(("sticky-db-locate", index))
                                                    .ghost()
                                                    .flex_1()
                                                    .label(scope.database.clone())
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.tree.scroll.scroll_to_item(
                                                                index,
                                                                ScrollStrategy::Top,
                                                            );
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                        move |menu, _, cx| {
                                            let owner = owner.clone();
                                            let scope = menu_scope.clone();
                                            menu.item(
                                                PopupMenuItem::new(t(cx, "database.note.new"))
                                                    .on_click(move |_, _, cx| {
                                                        let _ = owner.update(cx, |this, cx| {
                                                            this.open_scoped_note(
                                                                scope.id,
                                                                Some(scope.database.clone()),
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                        },
                                    )),
                            );
                        }
                        let Row::Connection(profile) = row else {
                            return body;
                        };
                        let id = profile.id;
                        let owner = cx.entity().downgrade();
                        body.child(
                            div()
                                .absolute()
                                .left_0()
                                .right(px(12.))
                                .top(px(top))
                                .h(px(32.))
                                .overflow_hidden()
                                .bg(cx.theme().background)
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(crate::context_menu::scoped(
                                    SharedString::from(format!("sticky-connection-{id}")),
                                    div()
                                        .h_flex()
                                        .size_full()
                                        .items_center()
                                        .child(
                                            Button::new(("sticky-toggle", index))
                                                .ghost()
                                                .icon(if self.tree_connection_open(id, cx) {
                                                    IconName::ChevronDown
                                                } else {
                                                    IconName::ChevronRight
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.tree_toggle_connection(id, window, cx)
                                                    },
                                                )),
                                        )
                                        .child(
                                            Button::new(("sticky-locate", index))
                                                .ghost()
                                                .flex_1()
                                                .min_w_0()
                                                .icon(super::navigation::type_icon(profile.kind))
                                                .label(profile.name.clone())
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.tree
                                                        .scroll
                                                        .scroll_to_item(index, ScrollStrategy::Top);
                                                    cx.notify();
                                                })),
                                        ),
                                    move |menu, _, cx| {
                                        if let Some(owner) = owner.upgrade() {
                                            owner.update(cx, |this, cx| {
                                                this.connection_menu(menu, id, false, cx)
                                            })
                                        } else {
                                            menu
                                        }
                                    },
                                )),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(px(12.))
                            .child(
                                gpui_component::scroll::Scrollbar::vertical(&self.tree.scroll)
                                    .scrollbar_show(gpui_component::scroll::ScrollbarShow::Always),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn tree_row(&self, row: Row, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Group(name) => self.connection_section(
                &name,
                name.clone(),
                vec![],
                false,
                &self.tree_connection_query(cx),
                cx,
            ),
            Row::Connection(p) => {
                let id = p.id;
                div()
                    .h_flex()
                    .w_full()
                    .items_center()
                    .child(
                        Button::new(SharedString::from(format!("tree-connection-{id}")))
                            .loading(self.note_connection_state(id).is_none())
                            .ghost()
                            .icon(if self.tree_connection_open(id, cx) {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.tree_toggle_connection(id, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.connection_row("merged", &p, cx)),
                    )
                    .into_any_element()
            }
            Row::Databases(scope) => self.tree_databases_row(scope, cx),
            Row::Users(scope) => self.tree_users_row(scope, cx),
            Row::UsersPage(scope) => self.tree_users_page_row(scope, cx),
            Row::User(scope, object) => self.tree_user_row(scope, object, cx),
            Row::DatabaseMore(id) => {
                let state = self.tree.databases.get(&id);
                let busy = state.is_some_and(|s| s.busy);
                let error = state.and_then(|s| s.error.as_ref());
                Button::new(SharedString::from(format!("database-page-{id}")))
                    .ghost()
                    .justify_start()
                    .pl(px(60.))
                    .child(if let Some(error) = error {
                        crate::locale::message(cx, error)
                    } else {
                        t(cx, if busy { "ddl.loading" } else { "browser.more" })
                    })
                    .child(
                        if busy {
                            gpui_component::Icon::new(IconName::LoaderCircle)
                        } else {
                            gpui_component::Icon::default().path("icons/load-more.svg")
                        }
                        .size_4(),
                    )
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.tree_database_page(id, true, cx)),
                    )
                    .into_any_element()
            }
            Row::DatabaseFilter(id) => {
                let Some(filter) = self.tree.database_filters.get(&id) else {
                    return div().into_any_element();
                };
                let input = filter.input.clone();
                div()
                    .pl(px(
                        if cx.global::<Preferences>().database.sidebar_layout
                            == crate::preferences::SidebarLayout::Split
                        {
                            0.
                        } else {
                            36.
                        },
                    ))
                    .pr_1()
                    .h_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Input::new(&input)
                            .cleanable(true)
                            .small()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        Button::new(SharedString::from(format!("database-rule-{id}")))
                            .ghost()
                            .small()
                            .icon(IconName::Settings2)
                            .selected(self.tree_database_rule(id).enabled)
                            .tooltip(t(cx, "database.rule.title"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.database_rule_dialog(id, window, cx)
                            })),
                    )
                    .capture_action(cx.listener(
                        move |this, _: &gpui_component::input::Escape, window, cx| {
                            if input.update(cx, |input, cx| {
                                input.marked_text_range(window, cx).is_some()
                            }) {
                                cx.propagate();
                                return;
                            }
                            if !input.read(cx).value().is_empty() {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            } else if let Some(filter) = this.tree.database_filters.get_mut(&id) {
                                filter.open = false;
                                this.focus.focus(window);
                                cx.notify();
                            }
                            cx.stop_propagation();
                        },
                    ))
                    .into_any_element()
            }
            Row::Database(scope) => {
                let key = scope.key("database");
                let open = self.navigation.tree_nodes.contains(&key);
                let selected = scope.clone();
                let owner = cx.entity().downgrade();
                let profile = self.note_profile(scope.id);
                let writable = profile.is_some_and(|p| p.supports_mutation());
                let create =
                    profile.is_some_and(|p| p.kind.can_create_database() && p.supports_mutation());
                let copy = writable
                    && profile.is_some_and(|p| {
                        matches!(
                            p.kind,
                            DatabaseKind::MySql
                                | DatabaseKind::PostgreSql
                                | DatabaseKind::Sqlite
                                | DatabaseKind::ClickHouse
                        )
                    });
                let button = Button::new(SharedString::from(key.clone()))
                    .loading(
                        self.tree
                            .pages
                            .iter()
                            .any(|((s, _), page)| s == &scope && !page.busy.is_empty()),
                    )
                    .ghost()
                    .w_full()
                    .justify_start()
                    .icon(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label(scope.database.clone())
                    .tooltip(scope.database.clone())
                    .pl(px(42.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.tree_select(&selected, cx) {
                            this.tree.filter.collapsed.remove(&key);
                            this.tree_toggle(key.clone(), cx);
                        }
                    }));
                let refresh_scope = scope.clone();
                let busy = self
                    .tree
                    .pages
                    .iter()
                    .any(|((s, _), p)| s == &scope && p.busy.contains("catalog"));
                let updated = self
                    .tree
                    .pages
                    .get(&page_key(&scope, "table"))
                    .map(|p| p.updated)
                    .unwrap_or(0);
                let button = div()
                    .h_flex()
                    .w_full()
                    .group("directory-database-row")
                    .child(button.flex_1().min_w_0())
                    .child(
                        Button::new(SharedString::from(scope.key("refresh-cache")))
                            .ghost()
                            .loading(busy)
                            .icon(if busy {
                                gpui_component::Icon::new(IconName::LoaderCircle)
                            } else {
                                crate::assets::refresh_icon()
                            })
                            .disabled(busy)
                            .when(!busy, |b| {
                                b.opacity(0.)
                                    .group_hover("directory-database-row", |style| {
                                        style.opacity(1.)
                                    })
                                    .focus(|style| style.opacity(1.))
                            })
                            .tooltip(if updated == 0 {
                                t(cx, "database.cache.refresh")
                            } else {
                                tr(
                                    cx,
                                    "database.cache.refresh_time",
                                    &[(
                                        "time",
                                        &chrono::DateTime::from_timestamp(updated, 0)
                                            .map(|time| {
                                                time.with_timezone(&chrono::Local)
                                                    .format("%Y-%m-%d %H:%M")
                                                    .to_string()
                                            })
                                            .unwrap_or_default(),
                                    )],
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tree_refresh_database(refresh_scope.clone(), cx)
                            })),
                    );
                crate::context_menu::scoped(
                    SharedString::from(scope.key("database-menu")),
                    button,
                    move |mut menu, _, cx| {
                        for (label, action, enabled) in [
                            ("dbwork.database.new", "create", create),
                            ("dbwork.database.copy", "copy", copy),
                            ("dbwork.import", "import", writable),
                            ("dbwork.export", "export", true),
                            ("database.note.new", "new-note", true),
                            ("dbwork.refresh", "refresh", true),
                        ] {
                            let owner = owner.clone();
                            let scope = scope.clone();
                            menu = menu.item(
                                PopupMenuItem::new(t(cx, label))
                                    .disabled(!enabled)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            if !this.tree_select(&scope, cx) {
                                                return;
                                            }
                                            match action {
                                                "new-note" => this.open_scoped_note(
                                                    scope.id,
                                                    Some(scope.database.clone()),
                                                    cx,
                                                ),
                                                "create" => this
                                                    .create_database_dialog(scope.id, window, cx),
                                                "copy" => this.transfer_dialog(
                                                    crate::transfer::Mode::Copy,
                                                    window,
                                                    cx,
                                                ),
                                                "import" => this.transfer_dialog(
                                                    crate::transfer::Mode::ImportFile,
                                                    window,
                                                    cx,
                                                ),
                                                "export" => this.transfer_dialog(
                                                    crate::transfer::Mode::Export,
                                                    window,
                                                    cx,
                                                ),
                                                _ => this.tree_refresh_database(scope.clone(), cx),
                                            }
                                        });
                                    }),
                            );
                        }
                        menu
                    },
                )
                .into_any_element()
            }
            Row::Category(scope, category) => {
                let loading = self
                    .tree
                    .pages
                    .get(&page_key(&scope, &category))
                    .is_some_and(|page| page.busy.contains("catalog"));
                let key = scope.key(&category);
                let open = self.tree_branch_open(&key, cx);
                let label = t(
                    cx,
                    match category.as_str() {
                        "table" => "dbwork.table",
                        "view" => "dbwork.view",
                        "index" => "dbwork.index",
                        "trigger" => "dbwork.trigger",
                        "procedure" => "dbwork.procedure",
                        "function" => "dbwork.function",
                        "event" => "dbwork.event",
                        _ => "dbwork.objects",
                    },
                );
                let owner = cx.entity().downgrade();
                let menu_scope = scope.clone();
                let table_category = category == "table";
                crate::context_menu::scoped(
                    SharedString::from(format!("cards-menu-{key}")),
                    Button::new(SharedString::from(key.clone()))
                        .loading(loading)
                        .ghost()
                        .w_full()
                        .justify_start()
                        .pl(px(58.))
                        .icon(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .label(label)
                        .on_mouse_down(MouseButton::Left, {
                            let scope = scope.clone();
                            let table = category == "table";
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                if table && event.click_count == 2 {
                                    this.open_table_cards(scope.id, scope.database.clone(), cx);
                                }
                            })
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if category == "index" && this.tree_select(&scope, cx) {
                                this.open_table(
                                    Object {
                                        schema: String::new(),
                                        name: String::new(),
                                        kind: "table".into(),
                                        comment: String::new(),
                                        parent: String::new(),
                                    },
                                    crate::table_view::Mode::Indexes,
                                    cx,
                                );
                            }
                            if this.tree_toggle_branch(key.clone(), cx)
                                && this.tree_object_query(cx).is_empty()
                                && !this
                                    .tree
                                    .pages
                                    .get(&page_key(&scope, &category))
                                    .is_some_and(|p| p.loaded)
                            {
                                this.tree_load(scope.clone(), category.clone(), false, cx);
                            }
                        })),
                    move |menu, _, cx| {
                        if !table_category {
                            return menu;
                        }
                        let owner = owner.clone();
                        let scope = menu_scope.clone();
                        menu.item(PopupMenuItem::new(t(cx, "database.cards.title")).on_click(
                            move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.open_table_cards(scope.id, scope.database.clone(), cx)
                                });
                            },
                        ))
                    },
                )
                .into_any_element()
            }
            Row::Object(scope, category, object) => {
                let key = scope.key(&Self::object_key(&object));
                let open = self.navigation.tree_nodes.contains(&key);
                let table = matches!(object.kind.as_str(), "table" | "view");
                let owner = cx.entity().downgrade();
                let menu_scope = scope.clone();
                let menu_object = object.clone();
                let writable = self
                    .note_profile(scope.id)
                    .is_some_and(|p| p.supports_mutation());
                let busy = self
                    .active_operation
                    .as_ref()
                    .is_some_and(|(_, _, kind)| !super::browser_ui::is_request(kind));
                let toggle_scope = scope.clone();
                let toggle_object = object.clone();
                div()
                    .h_flex()
                    .pl(px(54.))
                    .w_full()
                    .items_center()
                    .child(
                        Button::new(SharedString::from(key.clone()))
                            .loading(
                                self.tree
                                    .pages
                                    .get(&page_key(&scope, &category))
                                    .is_some_and(|page| {
                                        page.busy.contains(&Self::object_key(&object))
                                    }),
                            )
                            .ghost()
                            .icon(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .disabled(!table)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.tree_toggle(key.clone(), cx) {
                                    this.tree_columns(
                                        toggle_scope.clone(),
                                        category.clone(),
                                        toggle_object.clone(),
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        div().flex_1().min_w_0().child(crate::context_menu::scoped(
                            SharedString::from(
                                scope.key(&format!("menu:{}", Self::object_key(&object))),
                            ),
                            Button::new(SharedString::from(
                                scope.key(&format!("object:{}", Self::object_key(&object))),
                            ))
                            .ghost()
                            .justify_start()
                            .w_full()
                            .min_w_0()
                            .icon(IconName::GalleryVerticalEnd)
                            .label(object.name.clone())
                            .tooltip(format!("{} {}", object.schema, object.name))
                            .on_click(cx.listener(
                                move |this, event: &ClickEvent, window, cx| {
                                    if !this.tree_select(&scope, cx) {
                                        return;
                                    }
                                    if event.click_count() == 2 && object.kind == "index" {
                                        let mut parent = object.clone();
                                        parent.kind = "index".into();
                                        this.open_table(
                                            parent,
                                            crate::table_view::Mode::Indexes,
                                            cx,
                                        );
                                    } else if event.click_count() == 2 && table {
                                        this.tree_action(
                                            scope.clone(),
                                            object.clone(),
                                            Some(crate::table_view::Mode::Grid),
                                            "",
                                            window,
                                            cx,
                                        );
                                    } else {
                                        feedback::ddl_context(
                                            feedback::DdlContext {
                                                note: None,
                                                candidates: vec![],
                                                origin: None,
                                                scope: Some((scope.id, scope.database.clone())),
                                                object: Some(object.clone()),
                                                open: false,
                                            },
                                            cx,
                                        );
                                    }
                                },
                            )),
                            move |mut menu, _, cx| {
                                if !table {
                                    if menu_object.kind == "index" {
                                        return workbench_ui::index_menu_items(
                                            menu,
                                            owner.clone(),
                                            (
                                                menu_scope.id,
                                                menu_scope.database.clone(),
                                                menu_scope.generation,
                                            ),
                                            menu_object.clone(),
                                            cx,
                                        );
                                    }
                                    return menu;
                                }
                                for &(label, action, mode) in workbench_ui::TABLE_MENU_ACTIONS {
                                    if label.is_empty() {
                                        menu = menu.separator();
                                        continue;
                                    }
                                    let owner = owner.clone();
                                    let scope = menu_scope.clone();
                                    let object = menu_object.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(t(cx, label))
                                            .disabled(
                                                (busy && action != "name")
                                                    || (!writable
                                                        && matches!(
                                                            action,
                                                            "rename" | "delete" | "import"
                                                        )),
                                            )
                                            .on_click(move |_, window, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.tree_action(
                                                        scope.clone(),
                                                        object.clone(),
                                                        mode,
                                                        action,
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    );
                                }
                                menu
                            },
                        )),
                    )
                    .into_any_element()
            }
            Row::Column(scope, column) => {
                let name = column.name.clone();
                let kind = self.note_profile(scope.id).map(|profile| profile.kind);
                crate::context_menu::scoped(
                    SharedString::from(scope.key(&format!("column-menu:{name}"))),
                    div()
                        .h_flex()
                        .pl(px(108.))
                        .gap_2()
                        .text_sm()
                        .child(crate::table_view::column_icon(Some(&column), cx))
                        .child(div().flex_1().truncate().child(column.name))
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(column.data_type),
                        ),
                    move |menu, _, cx| crate::context_menu::column_copy(menu, &name, kind, cx),
                )
                .into_any_element()
            }
            Row::Page(scope, category) => {
                let page = self.tree.pages.get(&page_key(&scope, &category));
                let busy = page.is_some_and(|p| !p.busy.is_empty());
                let more = page.is_some_and(|p| p.more && p.query.is_empty());
                let retry = page.is_some_and(|page| page.error.is_some());
                let error = page
                    .and_then(|p| p.error.as_ref())
                    .map(|e| crate::locale::message(cx, e));
                div()
                    .pl(px(60.))
                    .h_flex()
                    .gap_1()
                    .min_w_0()
                    .when_some(error, |el, error| {
                        el.child(div().text_xs().truncate().child(error))
                    })
                    .child(
                        Button::new(SharedString::from(scope.key(&format!("load:{category}"))))
                            .ghost()
                            .icon(if busy {
                                gpui_component::Icon::new(IconName::LoaderCircle)
                            } else {
                                crate::assets::refresh_icon()
                            })
                            .label(t(
                                cx,
                                if busy {
                                    "ddl.loading"
                                } else if more {
                                    "browser.more"
                                } else if retry {
                                    "database.cache.retry"
                                } else {
                                    "database.filter.unindexed"
                                },
                            ))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tree_read(scope.clone(), category.clone(), more, retry, cx)
                            })),
                    )
                    .into_any_element()
            }
            Row::LayoutSettings(scope) => {
                Button::new(SharedString::from(scope.key("layout-settings")))
                    .ghost()
                    .icon(IconName::Settings2)
                    .label(t(cx, "database.sidebar.title"))
                    .tooltip(t(cx, "database.tree.unsupported"))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(crate::settings::SettingsRequest::Open(Some(
                            crate::settings::Category::Database,
                        )))
                    }))
                    .into_any_element()
            }
            Row::FilterMore(id) => self.tree_filter_more_row(id, cx),
            Row::FilterUnindexed(id) => self.tree_filter_unindexed_row(id, cx),
            Row::Notice(key) if matches!(key, "ddl.loading" | "database.branch.connecting") => {
                Button::new(SharedString::from(format!("tree-loading-{key}")))
                    .ghost()
                    .loading(true)
                    .label(t(cx, key))
                    .into_any_element()
            }
            Row::Notice(key) => div()
                .pl(px(
                    if cx.global::<Preferences>().database.sidebar_layout
                        == crate::preferences::SidebarLayout::Split
                    {
                        0.
                    } else {
                        38.
                    },
                ))
                .text_xs()
                .truncate()
                .child(t(cx, key))
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn cached_pages_are_isolated_by_connection_database_and_generation() {
        let a = Scope {
            id: Uuid::new_v4(),
            database: "first".into(),
            generation: 1,
        };
        let mut b = a.clone();
        b.database = "second".into();
        let mut reconnect = a.clone();
        reconnect.generation += 1;
        let mut other = a.clone();
        other.id = Uuid::new_v4();
        let mut tree = Tree::default();
        tree.pages.insert(
            page_key(&a, "table"),
            Page {
                loaded: true,
                ..Default::default()
            },
        );
        for scope in [b, reconnect, other] {
            assert!(!tree.pages.contains_key(&page_key(&scope, "table")));
        }
        assert_eq!(page_key(&a, "table"), page_key(&a, "view"));
        assert_ne!(page_key(&a, "table"), page_key(&a, "trigger"));
        // Expansion is persistent across reconnects, while metadata is never reused across generations.
        let mut current = a.clone();
        current.generation += 1;
        assert_eq!(a.key("table"), current.key("table"));
        let mut special = a.clone();
        special.database = "first\",\"table".into();
        assert_ne!(a.key("table"), special.key("table"));
    }
}

#[cfg(test)]
mod sticky_tests {
    #[test]
    fn header_follows_viewport_and_is_pushed_by_next_connection_or_project() {
        let markers = [(0, false), (1, true), (12, true), (20, false), (21, true)];
        assert_eq!(super::sticky_header(&markers, 32.), None);
        assert_eq!(super::sticky_header(&markers, 33.), Some((1, 0.)));
        assert_eq!(super::sticky_header(&markers, 368.), Some((1, -16.)));
        assert_eq!(super::sticky_header(&markers, 385.), Some((12, 0.)));
        assert_eq!(super::sticky_header(&markers, 641.), None);
        assert_eq!(super::sticky_header(&markers, 673.), Some((21, 0.)));
        assert_eq!(super::sticky_header(&[], 100.), None);
    }
}
