//! A local-only filter, independent of normal browsing and remote catalog pagination.
use super::*;
use crate::catalog_cache::SearchPage;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Request {
    query: String,
    sources: Vec<(Uuid, u64)>,
    catalog: u64,
    programs: bool,
    databases: Vec<(Uuid, String)>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum Phase {
    #[default]
    Idle,
    Waiting,
    Searching,
    Ready,
    Empty,
    Unindexed,
    Failed(String),
}
struct Results {
    request: Request,
    pages: HashMap<Uuid, SearchPage>,
}
#[derive(Default)]
pub(super) struct Filter {
    requested: Option<Request>,
    visible: Vec<(Uuid, String)>,
    sequence: u64,
    pub(super) phase: Phase,
    results: Option<Results>,
    task: Option<Task<()>>,
    fallback: Rc<Vec<Row>>,
    pub(super) collapsed: HashSet<String>,
    loading_more: HashSet<Uuid>,
    page_errors: HashMap<Uuid, String>,
}
impl Filter {
    fn retire_task(&mut self) {
        // A running local read must reach its guarded completion to record cancellation.
        // Pending debounce tasks have not logged a start and can simply be dropped.
        if let Some(task) = self.task.take()
            && self.phase == Phase::Searching
        {
            task.detach();
        }
    }
    pub(super) fn pending(&self) -> bool {
        matches!(self.phase, Phase::Waiting | Phase::Searching)
    }
    fn clear(&mut self) {
        self.sequence = self.sequence.wrapping_add(1);
        self.requested = None;
        self.retire_task();
        self.results = None;
        self.fallback = Rc::default();
        self.collapsed.clear();
        self.loading_more.clear();
        self.page_errors.clear();
        self.phase = Phase::Idle;
    }
    fn start(&mut self, request: Request, fallback: Rc<Vec<Row>>) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        self.requested = Some(request);
        self.retire_task();
        self.loading_more.clear();
        self.page_errors.clear();
        if self.results.is_none() {
            self.fallback = fallback;
        }
        self.phase = Phase::Waiting;
        self.sequence
    }
    fn accepts(&self, sequence: u64, request: &Request) -> bool {
        self.sequence == sequence && self.requested.as_ref() == Some(request)
    }
    fn finish(
        &mut self,
        sequence: u64,
        request: Request,
        result: Result<HashMap<Uuid, SearchPage>, String>,
    ) -> bool {
        if !self.accepts(sequence, &request) {
            return false;
        }
        match result {
            Ok(pages) => {
                if self.results.as_ref().is_none_or(|previous| {
                    previous.request.query != request.query
                        || previous.request.programs != request.programs
                }) {
                    self.collapsed.clear();
                }
                self.phase = if pages.values().any(|p| !p.hits.is_empty()) {
                    Phase::Ready
                } else if !pages.is_empty() && pages.values().all(|p| !p.indexed) {
                    Phase::Unindexed
                } else {
                    Phase::Empty
                };
                self.results = Some(Results { request, pages });
            }
            Err(error) => self.phase = Phase::Failed(error),
        }
        true
    }
}

impl DatabaseView {
    fn tree_filter_request(&self, cx: &App) -> Request {
        let mut sources = self
            .profiles
            .iter()
            .filter(|p| {
                p.kind != DatabaseKind::Redis
                    && self.note_connection_state(p.id) == Some(true)
                    && self.tree.filter.visible.iter().any(|(id, _)| *id == p.id)
            })
            .map(|p| (p.id, self.note_generation(p.id)))
            .collect::<Vec<_>>();
        sources.sort_unstable();
        Request {
            query: self.tree_object_query(cx),
            sources,
            catalog: self.tree.catalog_revision,
            programs: self.tree.search_scope == controls::SearchScope::Programs,
            databases: self
                .tree
                .filter
                .visible
                .iter()
                .filter(|(id, name)| {
                    let scope = Scope {
                        id: *id,
                        database: name.clone(),
                        generation: self.note_generation(*id),
                    };
                    let branch = Scope {
                        id: *id,
                        database: self
                            .note_profile(*id)
                            .map(|p| p.database.clone())
                            .unwrap_or_default(),
                        generation: scope.generation,
                    };
                    self.navigation.tree_nodes.contains(&scope.key("database"))
                        && !self
                            .tree
                            .filter
                            .collapsed
                            .contains(&branch.key("databases-closed"))
                        && !self
                            .tree
                            .filter
                            .collapsed
                            .contains(&format!("connection:{id}"))
                        && self.note_database_visible(*id, name)
                })
                .cloned()
                .collect(),
        }
    }
    pub(super) fn tree_filter_update(&mut self, cx: &mut Context<Self>) {
        if self.tree.filter.phase == Phase::Idle && !self.tree_object_query(cx).is_empty() {
            self.tree.filter.visible = self
                .tree
                .rows
                .iter()
                .filter_map(|row| {
                    if let Row::Database(scope) = row {
                        Some((scope.id, scope.database.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            self.tree.filter.visible.sort();
            self.tree.filter.visible.dedup();
        }
        let request = self.tree_filter_request(cx);
        if request.query.is_empty() {
            if self.tree.filter.phase != Phase::Idle {
                self.tree.filter.clear();
                self.tree.revision += 1;
            }
            return;
        }
        if self.tree.filter.requested.as_ref() == Some(&request) {
            return;
        }
        let sequence = self
            .tree
            .filter
            .start(request.clone(), self.tree.rows.clone());
        self.tree.revision += 1;
        if request.sources.is_empty() || request.databases.is_empty() {
            let pages = request
                .sources
                .iter()
                .map(|(id, _)| {
                    (
                        *id,
                        SearchPage {
                            complete: true,
                            indexed: true,
                            ..Default::default()
                        },
                    )
                })
                .collect();
            self.tree.filter.finish(sequence, request, Ok(pages));
            return;
        }
        // Freeze connected targets and the displayed database names before filtering.
        let profiles = request
            .sources
            .iter()
            .filter_map(|(id, _)| {
                self.note_profile(*id).map(|profile| {
                    (
                        *id,
                        crate::catalog_cache::Cache::new(&self.file, profile, "", "relations"),
                        self.tree_database_rule(*id),
                        request
                            .databases
                            .iter()
                            .filter(|(source, _)| source == id)
                            .map(|(_, name)| name.clone())
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect::<Vec<_>>();
        self.tree.filter.task = Some(cx.spawn(async move |view, cx| {
            gpui::Timer::after(std::time::Duration::from_millis(120)).await;
            let task = Uuid::new_v4();
            let started = std::time::Instant::now();
            let current = view
                .update(cx, |this, cx| {
                    if !this.tree.filter.accepts(sequence, &request)
                        || this.tree_filter_request(cx) != request
                    {
                        return false;
                    }
                    this.tree.filter.phase = Phase::Searching;
                    for (id, _) in &request.sources {
                        this.tree_cache_log(task, *id, started, None, false, cx);
                    }
                    this.tree.revision += 1;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let query = request.query.clone();
            let programs = request.programs;
            let result = cx
                .background_executor()
                .spawn(async move {
                    profiles
                        .into_iter()
                        .map(|(id, cache, rule, databases)| {
                            cache
                                .search_visible(&query, 0, 200, &rule, programs, Some(&databases))
                                .map(|page| (id, page))
                        })
                        .collect::<Result<HashMap<_, _>, _>>()
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                let current = this.tree_filter_request(cx) == request
                    && this.tree.filter.accepts(sequence, &request);
                let error = if current {
                    result.as_ref().err().map(String::as_str)
                } else {
                    Some("mcp.stale")
                };
                for (id, _) in &request.sources {
                    this.tree_cache_log(task, *id, started, error, true, cx);
                }
                if current && this.tree.filter.finish(sequence, request, result) {
                    this.tree.revision += 1;
                    cx.notify();
                }
            });
        }));
    }
    pub(super) fn tree_filter_rows(&self, cx: &App) -> Rc<Vec<Row>> {
        let current = self.tree_filter_request(cx);
        let Some(results) = &self.tree.filter.results else {
            // The first query retains the actual prior tree, without expanding or loading anything.
            let mut rows = vec![];
            let mut group = None;
            let mut allowed = false;
            for row in self.tree.filter.fallback.iter() {
                match row {
                    Row::Group(_) => {
                        group = Some(row.clone());
                        allowed = false;
                    }
                    Row::Connection(profile) => {
                        allowed = current
                            .sources
                            .contains(&(profile.id, self.note_generation(profile.id)));
                        if allowed {
                            if let Some(group) = group.take() {
                                rows.push(group);
                            }
                            rows.push(row.clone());
                        }
                    }
                    _ => {
                        if allowed {
                            rows.push(row.clone());
                        }
                    }
                }
            }
            return Rc::new(rows);
        };
        let mut rows = vec![];
        let search = self.tree_connection_query(cx);
        for group in self.ordered_groups() {
            let mut group_rows = vec![];
            for profile in self
                .profiles
                .iter()
                .filter(|p| self.group_key(p) == group && self.connection_matches(p, &search))
            {
                let id = profile.id;
                let generation = self.note_generation(id);
                if !current.sources.contains(&(id, generation))
                    || !results.request.sources.contains(&(id, generation))
                {
                    continue;
                }
                let Some(page) = results.pages.get(&id) else {
                    continue;
                };
                group_rows.push(Row::Connection(Rc::new(profile.clone())));
                if self
                    .tree
                    .filter
                    .collapsed
                    .contains(&format!("connection:{id}"))
                {
                    continue;
                }
                let mut databases = BTreeMap::<&str, Vec<&Object>>::new();
                for (source, name) in &self.tree.filter.visible {
                    if *source == id && self.note_database_visible(id, name) {
                        databases.entry(name).or_default();
                    }
                }

                for hit in page.hits.iter().filter(|hit| {
                    current.databases.contains(&(id, hit.database.clone()))
                        && self.note_database_visible(id, &hit.database)
                }) {
                    databases
                        .entry(&hit.database)
                        .or_default()
                        .push(&hit.object);
                }
                let branch = Scope {
                    id,
                    database: profile.database.clone(),
                    generation,
                };
                group_rows.push(Row::Databases(branch.clone()));
                if self
                    .tree
                    .filter
                    .collapsed
                    .contains(&branch.key("databases-closed"))
                {
                    continue;
                }
                for (database, objects) in databases {
                    let scope = Scope {
                        id,
                        database: database.into(),
                        generation,
                    };
                    group_rows.push(Row::Database(scope.clone()));
                    if !self.navigation.tree_nodes.contains(&scope.key("database"))
                        || self.tree.filter.collapsed.contains(&scope.key("database"))
                    {
                        continue;
                    }
                    for category in [
                        "table",
                        "view",
                        "index",
                        "procedure",
                        "trigger",
                        "function",
                        "event",
                    ] {
                        let objects = objects
                            .iter()
                            .filter(|o| o.kind == category)
                            .collect::<Vec<_>>();
                        if objects.is_empty() {
                            continue;
                        }
                        group_rows.push(Row::Category(scope.clone(), category.into()));
                        if self.tree.filter.collapsed.contains(&scope.key(category)) {
                            continue;
                        }
                        for object in objects {
                            group_rows.push(Row::Object(
                                scope.clone(),
                                category.into(),
                                (*object).clone(),
                            ));
                            let key = Self::object_key(object);
                            if self.navigation.tree_nodes.contains(&scope.key(&key))
                                && let Some(page) = self.tree.pages.get(&page_key(&scope, category))
                            {
                                if let Some(columns) = page.columns.get(&key) {
                                    group_rows.extend(columns.iter().cloned().map(Row::Column));
                                } else if page.busy.contains(&key) {
                                    group_rows.push(Row::Notice("ddl.loading"));
                                }
                            }
                        }
                    }
                }
                if page.more {
                    group_rows.push(Row::FilterMore(id));
                }
                if !page.complete {
                    group_rows.push(Row::FilterUnindexed(id));
                }
            }
            if !group_rows.is_empty() {
                rows.push(Row::Group(group));
                rows.extend(group_rows);
            }
        }
        Rc::new(rows)
    }
    pub(super) fn tree_filter_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let phase = &self.tree.filter.phase;
        let key = match phase {
            Phase::Waiting | Phase::Searching => Some("database.filter.searching"),
            Phase::Empty => Some(
                if self
                    .tree
                    .filter
                    .requested
                    .as_ref()
                    .is_some_and(|r| r.sources.is_empty())
                {
                    "database.filter.no_connections"
                } else if self
                    .tree
                    .filter
                    .requested
                    .as_ref()
                    .is_some_and(|r| r.databases.is_empty())
                {
                    "database.filter.expand_database"
                } else {
                    "database.filter.empty"
                },
            ),
            Phase::Unindexed => Some("database.filter.unindexed"),
            Phase::Failed(_) => Some("database.filter.failed"),
            _ => None,
        };
        let Some(key) = key else {
            return div().into_any_element();
        };
        div()
            .h_flex()
            .gap_1()
            .min_w_0()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                gpui_component::Icon::new(if matches!(phase, Phase::Failed(_)) {
                    IconName::TriangleAlert
                } else if self.tree.filter.pending() {
                    IconName::LoaderCircle
                } else {
                    IconName::Search
                })
                .size_3(),
            )
            .child(div().flex_1().min_w_0().truncate().child(t(cx, key)))
            .when(matches!(phase, Phase::Failed(_)), |row| {
                row.child(
                    Button::new("retry-local-tree-filter")
                        .ghost()
                        .small()
                        .icon(crate::assets::refresh_icon())
                        .label(t(cx, "database.filter.retry"))
                        .tooltip(if let Phase::Failed(error) = phase {
                            crate::locale::message(cx, error)
                        } else {
                            t(cx, "database.filter.retry")
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tree.filter.requested = None;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn tree_filter_more(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let filter = &mut self.tree.filter;
        if filter.pending() || filter.loading_more.contains(&id) {
            return;
        }
        let Some(results) = &filter.results else {
            return;
        };
        if filter.requested.as_ref() != Some(&results.request) {
            return;
        }
        let Some(page) = results.pages.get(&id).filter(|page| page.more) else {
            return;
        };
        let offset = page.hits.len();
        let request = results.request.clone();
        let sequence = filter.sequence;
        filter.loading_more.insert(id);
        filter.page_errors.remove(&id);
        let Some(profile) = self.note_profile(id) else {
            return;
        };
        let cache = crate::catalog_cache::Cache::new(&self.file, profile, "", "relations");
        let rule = self.tree_database_rule(id);
        let databases = request
            .databases
            .iter()
            .filter(|(source, _)| *source == id)
            .map(|(_, name)| name.clone())
            .collect::<Vec<_>>();
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.tree_cache_log(task, id, started, None, false, cx);
        cx.spawn(async move |view, cx| {
            let query = request.query.clone();
            let programs = request.programs;
            let result = cx
                .background_executor()
                .spawn(async move {
                    cache.search_visible(&query, offset, 200, &rule, programs, Some(&databases))
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                let current = this.tree_filter_request(cx) == request
                    && this.tree.filter.accepts(sequence, &request);
                this.tree_cache_log(
                    task,
                    id,
                    started,
                    if current {
                        result.as_ref().err().map(String::as_str)
                    } else {
                        Some("mcp.stale")
                    },
                    true,
                    cx,
                );
                if !current {
                    return;
                }
                this.tree.filter.loading_more.remove(&id);
                match result {
                    Ok(next) => {
                        if let Some(page) = this
                            .tree
                            .filter
                            .results
                            .as_mut()
                            .and_then(|r| r.pages.get_mut(&id))
                        {
                            page.hits.extend(next.hits);
                            page.more = next.more;
                            page.indexed = next.indexed;
                            page.complete = next.complete;
                        }
                    }
                    Err(error) => {
                        this.tree.filter.page_errors.insert(id, error);
                    }
                }
                this.tree.revision += 1;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn tree_filter_more_row(&self, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.tree.filter.pending() || self.tree.filter.loading_more.contains(&id);
        let current =
            self.tree.filter.results.as_ref().is_some_and(|results| {
                self.tree.filter.requested.as_ref() == Some(&results.request)
            });
        let error = self.tree.filter.page_errors.get(&id);
        Button::new(SharedString::from(format!("local-filter-more-{id}")))
            .ghost()
            .small()
            .pl(px(38.))
            .icon(if busy {
                IconName::LoaderCircle
            } else {
                IconName::ChevronDown
            })
            .label(t(
                cx,
                if busy {
                    "ddl.loading"
                } else if error.is_some() {
                    "database.filter.retry"
                } else {
                    "browser.more"
                },
            ))
            .when_some(error, |button, error| {
                button.tooltip(crate::locale::message(cx, error))
            })
            .disabled(busy || !current)
            .on_click(cx.listener(move |this, _, _, cx| this.tree_filter_more(id, cx)))
            .into_any_element()
    }
    pub(super) fn tree_filter_unindexed_row(&self, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        Button::new(SharedString::from(format!("local-filter-unindexed-{id}")))
            .ghost()
            .small()
            .pl(px(38.))
            .icon(IconName::Search)
            .label(t(cx, "database.filter.select_database"))
            .tooltip(t(cx, "database.filter.coverage"))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.note_connection_state(id) != Some(true) {
                    return;
                }
                this.tree_filter
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.navigation.tree_connections.insert(id);
                this.persist_navigation();
                cx.notify();
            }))
            .into_any_element()
    }
    pub(super) fn tree_connection_open(&self, id: Uuid, cx: &App) -> bool {
        if !self.tree_object_query(cx).is_empty() && self.tree.filter.results.is_some() {
            !self
                .tree
                .filter
                .collapsed
                .contains(&format!("connection:{id}"))
        } else {
            self.navigation.tree_connections.contains(&id)
        }
    }
    pub(super) fn tree_toggle_connection(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if !self.tree_object_query(cx).is_empty() {
            let key = format!("connection:{id}");
            if !self.tree.filter.collapsed.remove(&key) {
                self.tree.filter.collapsed.insert(key);
            }
        } else {
            if !self.navigation.tree_connections.remove(&id) {
                self.navigation.tree_connections.insert(id);
            }
            self.persist_navigation();
        }
        self.tree.revision += 1;
        cx.notify();
    }
    pub(super) fn tree_branch_open(&self, key: &str, cx: &App) -> bool {
        if self.tree_object_query(cx).is_empty() {
            self.navigation.tree_nodes.contains(key)
        } else if self.tree.filter.results.is_some() {
            !self.tree.filter.collapsed.contains(key)
        } else {
            self.navigation.tree_nodes.contains(key)
        }
    }
    pub(super) fn tree_toggle_branch(&mut self, key: String, cx: &mut Context<Self>) -> bool {
        if self.tree_object_query(cx).is_empty() {
            return self.tree_toggle(key, cx);
        }
        let open = if self.tree.filter.collapsed.remove(&key) {
            true
        } else {
            self.tree.filter.collapsed.insert(key);
            false
        };
        self.tree.revision += 1;
        cx.notify();
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(query: &str) -> Request {
        Request {
            query: query.into(),
            sources: vec![(Uuid::nil(), 1)],
            catalog: 0,
            programs: false,
            databases: vec![],
        }
    }
    #[::core::prelude::v1::test]
    fn stale_queries_and_errors_do_not_replace_a_visible_result() {
        let mut filter = Filter::default();
        let a = request("a");
        let first = filter.start(a.clone(), Rc::default());
        let pages = HashMap::from([(
            Uuid::nil(),
            SearchPage {
                indexed: true,
                ..Default::default()
            },
        )]);
        assert!(filter.finish(first, a.clone(), Ok(pages)));
        let next = filter.start(request("ab"), Rc::default());
        assert!(filter.pending());
        assert_eq!(filter.results.as_ref().unwrap().request, a);
        assert!(!filter.finish(first, a.clone(), Ok(HashMap::new())));
        assert!(filter.finish(
            next,
            request("ab"),
            Err("database.cache.storage_error".into())
        ));
        assert!(matches!(filter.phase, Phase::Failed(_)));
        assert_eq!(filter.results.as_ref().unwrap().request, a);
        filter.clear();
        let later = filter.start(a.clone(), Rc::default());
        assert_ne!(later, first);
        assert!(!filter.finish(first, a, Ok(HashMap::new())));
    }
    #[::core::prelude::v1::test]
    fn missing_cache_is_distinct_from_an_indexed_empty_result() {
        let mut filter = Filter::default();
        let request = request("x");
        let sequence = filter.start(request.clone(), Rc::default());
        filter.finish(
            sequence,
            request.clone(),
            Ok(HashMap::from([(Uuid::nil(), SearchPage::default())])),
        );
        assert_eq!(filter.phase, Phase::Unindexed);
        let sequence = filter.start(request.clone(), Rc::default());
        filter.finish(
            sequence,
            request,
            Ok(HashMap::from([(
                Uuid::nil(),
                SearchPage {
                    indexed: true,
                    complete: true,
                    ..Default::default()
                },
            )])),
        );
        assert_eq!(filter.phase, Phase::Empty);
    }
    #[::core::prelude::v1::test]
    fn reconnect_and_snapshot_changes_retire_pending_requests() {
        let mut filter = Filter::default();
        let old = request("x");
        let sequence = filter.start(old.clone(), Rc::default());
        let mut changed = old.clone();
        changed.sources[0].1 += 1;
        filter.start(changed, Rc::default());
        assert!(!filter.finish(sequence, old, Ok(HashMap::new())));
        let old = filter.requested.clone().unwrap();
        let sequence = filter.sequence;
        let mut changed = old.clone();
        changed.catalog += 1;
        filter.start(changed, Rc::default());
        assert!(!filter.finish(sequence, old, Ok(HashMap::new())));
    }
}
