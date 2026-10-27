use super::*;
use crate::{
    locale::{t, tr},
    object_browser::{Browser, PageState, bucket},
};
impl DatabaseView {
    pub(super) fn browser_filter_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.object_filter.read(cx).value().trim().to_lowercase();
        if query == self.browser.query {
            cx.notify();
            return;
        }
        self.browser.query = query.clone();
        self.browser.timer = None;
        if self.browser.supported {
            let context = self.selected_database.clone();
            let generation = context.as_ref().map(|(id, _)| self.note_generation(*id));
            self.browser.timer = Some(cx.spawn(async move |view, cx| {
                gpui::Timer::after(std::time::Duration::from_millis(400)).await;
                let _ = view.update(cx, |this, cx| {
                    this.browser.timer = None;
                    if this.selected_database != context
                        || context.as_ref().map(|(id, _)| this.note_generation(*id)) != generation
                        || this.object_filter.read(cx).value().trim().to_lowercase() != query
                    {
                        return;
                    }
                    let categories = this.browser_search_categories(cx);
                    this.cancel_stale_metadata(&query, cx);
                    this.browser.search(query, &categories);
                    this.pump_browser(cx);
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }
    pub(super) fn restart_browser_search(&mut self, cx: &mut Context<Self>) {
        if self.browser.session.is_some()
            && matches!(self.browser.pending, Some(Action::BrowseCatalog { .. }))
        {
            self.close_metadata();
            self.browser.pending = None;
        }
        let query = self.object_filter.read(cx).value().trim().to_lowercase();
        let categories = self.browser_search_categories(cx);
        self.browser.search(query, &categories);
        self.pump_browser(cx);
        cx.notify();
    }
    pub(super) fn browser_search_categories(&self, cx: &App) -> Vec<&'static str> {
        self.browser_categories(cx)
            .into_iter()
            .filter(|c| {
                self.browser.all_categories
                    || matches!(*c, "table" | "view")
                    || self.expanded_objects.contains(&format!("root-open:{c}"))
            })
            .collect()
    }
    pub(crate) fn paged_catalog_supported(&self, id: Uuid) -> bool {
        self.note_profile(id).is_some_and(|p| {
            matches!(
                p.kind,
                DatabaseKind::MySql
                    | DatabaseKind::PostgreSql
                    | DatabaseKind::Sqlite
                    | DatabaseKind::ClickHouse
            )
        }) && self
            .info
            .get(&id)
            .is_none_or(|i| !i.version.starts_with("4."))
    }
    pub(super) fn browser_categories(&self, _cx: &App) -> Vec<&'static str> {
        let Some((id, _)) = self.selected_database.as_ref() else {
            return vec![];
        };
        let Some(p) = self.profiles.iter().find(|p| p.id == *id) else {
            return vec![];
        };
        let version = self
            .info
            .get(id)
            .map(|i| i.version.as_str())
            .unwrap_or_default();
        workbench::categories(p.kind, version)
    }
    pub(super) fn pump_browser(&mut self, cx: &mut Context<Self>) {
        if self.browser.pending.is_some() {
            return;
        }
        // Read-only metadata uses a separate session; foreground work never waits for it.
        if self
            .browser
            .queue
            .front()
            .is_some_and(|a| !matches!(a, Action::Describe(_)))
            && !self
                .selected_database
                .as_ref()
                .and_then(|(id, _)| self.connected_profiles.get(id))
                .is_some_and(|p| {
                    p.kind == DatabaseKind::Sqlite
                        && (p.sqlite_path == ":memory:" || p.sqlite_path.contains("mode=memory"))
                })
        {
            self.pump_metadata(cx);
            return;
        }
        if !self.mcp_queue.is_empty() {
            self.pump_mcp(cx);
            return;
        }
        if !self.browser.supported || self.active_operation.is_some() {
            return;
        }
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        let Some(action) = self.browser.queue.pop_front() else {
            return;
        };
        self.browser.queued.remove(&Browser::token(&action));
        if let Action::BrowseCatalog { search, .. } = &action
            && !search.is_empty()
            && *search != self.object_filter.read(cx).value().trim().to_lowercase()
        {
            cx.notify();
            return;
        }
        if self.request_operation(
            id,
            OperationKind::Workbench(Box::new(workbench::Request {
                database: db,
                action: action.clone(),
            })),
            cx,
        ) {
            self.browser.pending = Some(action);
            self.browser.revision += 1;
        }
    }
    pub(super) fn browser_columns(&mut self, object: Object, cx: &mut Context<Self>) {
        if !self.browser.supported {
            self.describe_object(object, cx);
            return;
        }
        let key = Self::object_key(&object);
        if self.browser.columns.contains(&key) {
            return;
        }
        if let Some((id, db)) = self.selected_database.clone()
            && let Some(columns) = self.cached_note_columns(id, &db, &object)
        {
            self.schemas.entry(key.clone()).or_default().columns = columns;
            self.browser.columns.insert(key);
            self.browser.revision += 1;
            return;
        }
        self.prioritize_metadata_columns(cx);
        self.browser.push(Action::BrowseColumns(object));
        self.pump_browser(cx);
    }
    pub(super) fn browser_section(
        &mut self,
        object: Object,
        section: String,
        cx: &mut Context<Self>,
    ) {
        if !self.browser.supported {
            return;
        }
        let key = format!("{}:{section}", Self::object_key(&object));
        if !self.browser.sections.contains(&key) {
            self.browser.push(Action::BrowseSection { object, section });
            self.pump_browser(cx);
        }
    }
    pub(super) fn browser_full(&mut self, object: Object, cx: &mut Context<Self>) {
        if self.browser.supported {
            self.browser.push(Action::Describe(object));
            self.pump_browser(cx);
        } else {
            self.describe_object(object, cx);
        }
    }
    pub(super) fn browser_state_row(&self, action: Action, cx: &Context<Self>) -> AnyElement {
        let token = Browser::token(&action);
        let pending = self
            .browser
            .pending
            .iter()
            .chain(self.browser.queue.iter())
            .any(|a| Browser::token(a) == token);
        let error = self.browser.errors.get(&token);
        let running = self
            .browser
            .pending
            .as_ref()
            .is_some_and(|a| Browser::token(a) == token)
            && self.browser.sent;
        let label = if pending {
            t(
                cx,
                if running {
                    "browser.loading"
                } else {
                    "browser.waiting"
                },
            )
            .to_string()
        } else if let Some(error) = error {
            format!(
                "{} · {}",
                crate::locale::message(cx, error),
                t(cx, "browser.retry")
            )
        } else {
            t(cx, "browser.load").to_string()
        };
        div()
            .pl(px(40.))
            .child(
                Button::new(SharedString::from(token))
                    .ghost()
                    .label(label)
                    .icon(if pending {
                        IconName::LoaderCircle
                    } else {
                        IconName::Redo
                    })
                    .disabled(pending)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser.push(action.clone());
                        this.pump_browser(cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    pub(super) fn receive_browser(
        &mut self,
        id: Uuid,
        kind: &OperationKind,
        result: &Result<OperationResult, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let OperationKind::Workbench(request) = kind else {
            return false;
        };
        let a = &request.action;
        let handled = matches!(
            a,
            Action::BrowseCatalog { .. } | Action::BrowseColumns(_) | Action::BrowseSection { .. }
        );
        if self
            .browser
            .pending
            .as_ref()
            .is_some_and(|p| Browser::token(p) == Browser::token(a))
        {
            self.browser.pending = None;
            self.browser.revision += 1;
        }
        if !handled {
            return false;
        }
        if self
            .selected_database
            .as_ref()
            .is_none_or(|(current, db)| *current != id || *db != request.database)
        {
            return true;
        }
        let current = self.object_filter.read(cx).value().trim().to_lowercase();
        if matches!(a,Action::BrowseCatalog{search,..} if !search.is_empty()&&*search!=current) {
            return true;
        }
        match result {
            Ok(OperationResult::Workbench(reply)) => match (a, reply.as_ref()) {
                (
                    Action::BrowseCatalog {
                        category,
                        search,
                        offset,
                        ..
                    },
                    Reply::CatalogPage { objects, more },
                ) => {
                    self.browser
                        .merge(&mut self.catalog_objects, objects.clone());
                    self.browser.pages.insert(
                        (category.clone(), search.clone()),
                        PageState {
                            loaded: true,
                            more: *more,
                            offset: offset + objects.len(),
                        },
                    );
                    self.object_catalog_loaded = true;
                    self.object_notice = tr(
                        cx,
                        "browser.loaded",
                        &[("count", &self.catalog_objects.len().to_string())],
                    )
                    .to_string();
                }
                (Action::BrowseColumns(object), Reply::Schema(schema)) => {
                    let key = Self::object_key(object);
                    self.browser_trim_schema(&key);
                    self.schemas.entry(key.clone()).or_default().columns = schema.columns.clone();
                    self.browser.columns.insert(key);
                    self.remember_note_columns(id, &request.database, object, &schema.columns);
                    cx.emit(NoteColumnsReady(id, None));
                }
                (Action::BrowseSection { object, section }, Reply::Schema(schema)) => {
                    let key = Self::object_key(object);
                    self.browser_trim_schema(&key);
                    let saved = self.schemas.entry(key.clone()).or_default();
                    match section.as_str() {
                        "dbwork.index" => saved.indexes = schema.indexes.clone(),
                        "dbwork.foreign" => saved.foreign_keys = schema.foreign_keys.clone(),
                        _ => saved.triggers = schema.triggers.clone(),
                    };
                    self.browser.sections.insert(format!("{key}:{section}"));
                }
                _ => {
                    self.browser
                        .errors
                        .insert(Browser::token(a), "browser.response_failed".into());
                }
            },
            Err(error) => {
                self.browser.errors.insert(Browser::token(a), error.clone());
            }
            _ => {
                self.browser
                    .errors
                    .insert(Browser::token(a), "browser.response_failed".into());
            }
        }
        self.browser.revision += 1;
        true
    }
    pub(crate) fn remember_designer_schema(
        &mut self,
        target: &OpenTable,
        schema: &workbench::Schema,
        cx: &mut Context<Self>,
    ) {
        if self.note_generation(target.connection) != target.generation {
            return;
        }
        self.remember_note_columns(
            target.connection,
            &target.database,
            &target.object,
            &schema.columns,
        );
        if self.selected_database.as_ref() == Some(&(target.connection, target.database.clone())) {
            let key = Self::object_key(&target.object);
            self.browser_trim_schema(&key);
            self.schemas.insert(key.clone(), schema.clone());
            self.browser.columns.insert(key.clone());
            self.browser.full.insert(key.clone());
            for section in ["dbwork.index", "dbwork.foreign"] {
                self.browser.sections.insert(format!("{key}:{section}"));
            }
            if target.kind != DatabaseKind::MySql {
                self.browser
                    .sections
                    .insert(format!("{key}:dbwork.trigger"));
            }
            self.browser.revision += 1;
        }
        cx.notify();
    }
    pub(super) fn browser_trim_schema(&mut self, keep: &str) {
        if self.schemas.len() >= 32
            && !self.schemas.contains_key(keep)
            && let Some(key) = self
                .schemas
                .keys()
                .find(|k| !self.expanded_objects.contains(*k))
                .or_else(|| self.schemas.keys().next())
                .cloned()
        {
            self.schemas.remove(&key);
            self.browser.columns.remove(&key);
            self.browser.full.remove(&key);
            self.browser
                .sections
                .retain(|k| !k.starts_with(&format!("{key}:")));
        }
    }
    pub(super) fn browser_page_row(
        &self,
        category: &str,
        query: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let category = bucket(category).to_string();
        let page = self.browser.pages.get(&(category.clone(), query.into()));
        let loading = self.browser.loading(&category, query);
        if loading {
            return div()
                .pl(px(20.))
                .h(px(30.))
                .text_sm()
                .child(t(cx, if self.browser.sent && matches!(&self.browser.pending, Some(Action::BrowseCatalog {category: c, search, ..}) if c == &category && search == query) { "browser.searching" } else { "browser.waiting" }))
                .into_any_element();
        }
        let offset = page.map_or(0, |p| p.offset);
        let action = Action::BrowseCatalog {
            category: category.clone(),
            search: query.into(),
            offset,
            limit: turbodbn_services::database::catalog_browser::PAGE,
            include_comments: self.browser.include_comments,
        };
        let error = self.browser.errors.contains_key(&Browser::token(&action));
        if error {
            return self.browser_state_row(action, cx);
        }
        let more = page.is_some_and(|p| p.more);
        let at_limit = offset >= turbodbn_services::database::catalog_browser::MAX_CACHE;
        let label = if at_limit {
            t(cx, "browser.limit")
        } else if more {
            t(cx, "browser.more")
        } else {
            t(cx, "browser.load")
        };
        Button::new(SharedString::from(format!("browser-more:{category}")))
            .ghost()
            .label(label)
            .icon(IconName::ChevronDown)
            .disabled(at_limit)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.browser.push(action.clone());
                this.pump_browser(cx);
                cx.notify();
            }))
            .into_any_element()
    }
}

impl DatabaseView {
    pub(super) fn render_browser_list(
        &mut self,
        body: Div,
        categories: &[&'static str],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::object_browser::TreeRow;
        if self.active_operation.is_none()
            && self.browser.pending.is_none()
            && !self.browser.queue.is_empty()
        {
            cx.defer_in(_window, |this, _, cx| this.pump_browser(cx));
        }
        if self.objects_accounts_need_load() {
            cx.defer_in(_window, |this, _, cx| this.objects_show_accounts(cx));
        }
        let query = self.object_filter.read(cx).value().trim().to_lowercase();
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for key in &self.expanded_objects {
            key.hash(&mut hasher);
        }
        self.objects_accounts_revision().hash(&mut hasher);
        self.selected_database.hash(&mut hasher);
        let fingerprint = (self.browser.revision, query.clone(), hasher.finish());
        if self.browser.fingerprint.as_ref() != Some(&fingerprint) {
            self.browser.counts.clear();
            let mut rows = Vec::new();
            let mut shown = std::collections::HashSet::new();
            for category in categories {
                let members = self
                    .catalog_objects
                    .iter()
                    .filter(|o| o.kind == *category && self.browser.matches(o, &query))
                    .cloned()
                    .collect::<Vec<_>>();
                self.browser
                    .counts
                    .insert((*category).into(), members.len());
                let group = bucket(category);
                let page = self
                    .browser
                    .pages
                    .get(&(group.into(), query.clone()))
                    .or_else(|| {
                        self.browser
                            .pages
                            .get(&(group.into(), String::new()))
                            .filter(|p| p.loaded && !p.more)
                    });
                let pending = self.browser.loading(group, &query);
                if !query.is_empty()
                    && members.is_empty()
                    && !pending
                    && page.is_some_and(|p| p.loaded && !p.more)
                {
                    continue;
                }
                rows.push(TreeRow::Category(category));
                let folded = if *category == "table" {
                    self.expanded_objects.contains(*category)
                } else {
                    !self
                        .expanded_objects
                        .contains(&format!("root-open:{category}"))
                };
                if !folded {
                    rows.extend(members.into_iter().map(TreeRow::Object));
                    if self.browser.supported
                        && shown.insert(group.to_string())
                        && (page.is_none_or(|p| !p.loaded || p.more) || pending)
                    {
                        rows.push(TreeRow::Page(category.to_string()));
                    }
                }
            }
            rows.extend(self.objects_account_rows());
            let keys = rows.iter().map(TreeRow::key).collect::<Vec<_>>();
            if self.browser.layout != keys {
                let prefix = self
                    .browser
                    .layout
                    .iter()
                    .zip(&keys)
                    .take_while(|(a, b)| a == b)
                    .count();
                let suffix = self.browser.layout[prefix..]
                    .iter()
                    .rev()
                    .zip(keys[prefix..].iter().rev())
                    .take_while(|(a, b)| a == b)
                    .count();
                self.browser.list.splice(
                    prefix..self.browser.layout.len() - suffix,
                    keys.len() - prefix - suffix,
                );
                self.browser.layout = keys;
            }
            // Only expanded object groups can change height between metadata responses.
            for (i, row) in rows.iter().enumerate() {
                if matches!(row,TreeRow::Object(o) if self.expanded_objects.contains(&Self::object_key(o)))
                {
                    self.browser.list.splice(i..i + 1, 1);
                }
            }
            self.browser.rows = std::rc::Rc::new(rows);
            self.browser.fingerprint = Some(fingerprint);
        }
        let rows = self.browser.rows.clone();
        let matches = self.browser.counts.values().sum::<usize>();
        let state = self.browser.list.clone();
        let scroll = state.clone();
        let searching = self.browser.timer.is_some()
            || self
                .browser
                .pending
                .iter()
                .chain(self.browser.queue.iter())
                .any(|a| matches!(a, Action::BrowseCatalog { .. }));
        let label = if !self.browser.supported {
            if self.browser.unknown_limit {
                t(cx, "browser.local_only").to_string()
            } else {
                self.object_notice.clone()
            }
        } else if searching {
            t(cx, "browser.searching").to_string()
        } else if self
            .browser
            .errors
            .keys()
            .any(|key| key.starts_with("page:"))
        {
            t(cx, "browser.incomplete").to_string()
        } else if !query.is_empty() && matches == 0 {
            t(cx, "browser.no_match").to_string()
        } else {
            tr(
                cx,
                "browser.loaded",
                &[("count", &self.catalog_objects.len().to_string())],
            )
            .to_string()
        };
        body.child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(
                    gpui::list(
                        state,
                        cx.processor(move |this, index: usize, window, cx| {
                            match rows[index].clone() {
                                row @ (TreeRow::Accounts
                                | TreeRow::Account(_)
                                | TreeRow::AccountsPage
                                | TreeRow::AccountsEmpty) => this.objects_account_row(row, cx),
                                TreeRow::Object(object) => {
                                    this.render_browser_object(object, window, cx)
                                }
                                TreeRow::Page(category) => this.browser_page_row(
                                    &category,
                                    &this.object_filter.read(cx).value().trim().to_lowercase(),
                                    cx,
                                ),
                                TreeRow::Category(category) => {
                                    let query =
                                        this.object_filter.read(cx).value().trim().to_lowercase();
                                    let group = bucket(category);
                                    let folded = if category == "table" {
                                        this.expanded_objects.contains(category)
                                    } else {
                                        !this
                                            .expanded_objects
                                            .contains(&format!("root-open:{category}"))
                                    };
                                    let count =
                                        this.browser.counts.get(category).copied().unwrap_or(0);
                                    let page = this
                                        .browser
                                        .pages
                                        .get(&(group.into(), query.clone()))
                                        .or_else(|| {
                                            this.browser
                                                .pages
                                                .get(&(group.into(), String::new()))
                                                .filter(|p| p.loaded && !p.more)
                                        });
                                    let count = if this.browser.supported && page.is_none() {
                                        t(cx, "browser.not_loaded").to_string()
                                    } else {
                                        format!(
                                            "{count}{}",
                                            if page.is_some_and(|p| p.more) {
                                                "+"
                                            } else {
                                                ""
                                            }
                                        )
                                    };
                                    let label = match category {
                                        "table" => "dbwork.table",
                                        "view" => "dbwork.view",
                                        "index" => "dbwork.index",
                                        "trigger" => "dbwork.trigger",
                                        "procedure" => "dbwork.procedure",
                                        "function" => "dbwork.function",
                                        _ => "dbwork.event",
                                    };
                                    Button::new(SharedString::from(format!(
                                        "object-root-{category}"
                                    )))
                                    .ghost()
                                    .icon(if folded {
                                        IconName::ChevronRight
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .label(format!("{} ({count})", t(cx, label)))
                                    .justify_start()
                                    .w_full()
                                    .h(px(30.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if category == "index" {
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
                                        let key = if category == "table" {
                                            category.into()
                                        } else {
                                            format!("root-open:{category}")
                                        };
                                        if !this.expanded_objects.remove(&key) {
                                            this.expanded_objects.insert(key.clone());
                                        }
                                        let open = if category == "table" {
                                            !this.expanded_objects.contains(&key)
                                        } else {
                                            this.expanded_objects.contains(&key)
                                        };
                                        let query = this
                                            .object_filter
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_lowercase();
                                        if open
                                            && this.browser.supported
                                            && !this
                                                .browser
                                                .pages
                                                .get(&(bucket(category).into(), query.clone()))
                                                .is_some_and(|p| p.loaded)
                                        {
                                            this.browser.page(bucket(category), &query, false);
                                            this.pump_browser(cx);
                                        }
                                        cx.notify();
                                    }))
                                    .into_any_element()
                                }
                            }
                        }),
                    )
                    .size_full(),
                )
                .child(
                    gpui_component::scroll::Scrollbar::vertical(&scroll)
                        .scrollbar_show(gpui_component::scroll::ScrollbarShow::Always),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .into_any_element()
    }
}

pub(super) fn is_request(kind: &OperationKind) -> bool {
    matches!(kind,OperationKind::Workbench(r) if matches!(r.action,Action::BrowseCatalog{..}|Action::BrowseColumns(_)|Action::BrowseSection{..}))
}
