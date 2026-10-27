use super::*;
impl Workspace {
    fn reconcile_center(&mut self) {
        if self.center_groups.is_empty() {
            self.center_groups.push(vec![]);
        }
        self.active_center = self.active_center.min(self.center_groups.len() - 1);
        for group in &mut self.center_groups {
            group.retain(|name| self.center_order.contains(name));
        }
        for name in &self.center_order {
            if !self.center_groups.iter().any(|g| g.contains(name)) {
                self.center_groups[self.active_center].push(name.clone());
            }
        }
    }
    pub(super) fn activate_center(&mut self, name: &str, cx: &App) {
        self.reconcile_center();
        if let Some(i) = name
            .strip_prefix("empty-center-")
            .and_then(|i| i.parse::<usize>().ok())
            .filter(|i| *i < self.center_views.len())
        {
            self.active_center = i;
            self.center_tabs = self.center_views[i].clone();
            return;
        }
        if !self.center_order.iter().any(|n| n == name)
            && let Some(i) = self
                .restored_center
                .iter()
                .position(|group| group.iter().any(|n| n == name))
                .filter(|i| *i < self.center_views.len())
        {
            self.active_center = i;
            self.center_tabs = self.center_views[i].clone();
            return;
        }
        if let Some(i) = self
            .center_groups
            .iter()
            .position(|g| g.iter().any(|n| n == name))
        {
            self.active_center = i;
            if let Some(view) = self.center_views.get(i) {
                self.center_tabs = view.clone();
            }
        }
        let _ = cx;
    }
    pub(super) fn panel_for(&self, name: &str, cx: &App) -> Option<Arc<dyn PanelView>> {
        self.note_views
            .get(name)
            .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
            .or_else(|| {
                self.permission_panels
                    .get(name)
                    .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                    .or_else(|| {
                        self.note_panels
                            .get(name)
                            .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                    })
                    .or_else(|| {
                        self.table_panels
                            .get(name)
                            .map(|p| Arc::new(p.clone()) as Arc<dyn PanelView>)
                    })
                    .or_else(|| {
                        self.panels
                            .iter()
                            .find(|p| p.panel_name(cx) == name)
                            .cloned()
                    })
            })
    }
    pub(super) fn rebuild_center(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reconcile_center();
        cx.global_mut::<crate::tabs::TabRouter>().1 = self.center_groups.len();
        let active_panels = self
            .center_views
            .iter()
            .filter_map(|view| view.read(cx).active_panel(cx))
            .map(|p| p.view().entity_id())
            .collect::<std::collections::HashSet<_>>();
        let mut items = vec![];
        self.center_views.clear();
        for (i, group) in self.center_groups.iter().enumerate() {
            let mut panels: Vec<Arc<dyn PanelView>> =
                group.iter().filter_map(|n| self.panel_for(n, cx)).collect();
            if panels.is_empty() {
                panels.push(Arc::new(cx.new(|cx| crate::tabs::EmptyPane::new(i, cx))));
            }

            let active = panels
                .iter()
                .position(|p| active_panels.contains(&p.view().entity_id()))
                .unwrap_or(0);
            let item =
                DockItem::tabs_with_active(panels, active, &self.dock.downgrade(), window, cx);
            if let DockItem::Tabs { view, .. } = &item {
                self.center_views.push(view.clone());
            }
            items.push(item);
        }
        self.center_tabs = self.center_views[self.active_center].clone();
        let item = if items.len() == 1 {
            items.remove(0)
        } else {
            DockItem::split_with_sizes(
                Axis::Horizontal,
                items,
                (0..self.center_groups.len())
                    .map(|i| self.center_sizes.get(i).copied().map(px))
                    .collect(),
                &self.dock.downgrade(),
                window,
                cx,
            )
        };
        self.dock
            .update(cx, |dock, cx| dock.set_center(item, window, cx));
        self.save_center_layout(cx);
        cx.notify();
    }
    pub(super) fn change_center(
        &mut self,
        event: crate::tabs::LayoutAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::tabs::LayoutAction;
        self.reconcile_center();
        let old_count = self.center_groups.len();
        match event {
            LayoutAction::Activate(name) => {
                self.activate_center(&name, cx);
                return;
            }
            LayoutAction::Select(name) => {
                self.select_note(&name, window, cx);
                return;
            }
            LayoutAction::Restore(origin) => {
                if self.center_groups.len() <= 1 {
                    return;
                }
                let origin = self
                    .note_views
                    .get(&origin)
                    .map(|v| v.read(cx).owner.read(cx).key.clone())
                    .unwrap_or(origin);
                let mut merged = vec![];
                for name in self.center_groups.iter().flatten() {
                    let key = self
                        .note_views
                        .get(name)
                        .map(|v| v.read(cx).owner.read(cx).key.clone())
                        .unwrap_or_else(|| name.clone());
                    if !merged.contains(&key) {
                        merged.push(key);
                    }
                }
                let views = std::mem::take(&mut self.note_views);
                for (key, view) in views {
                    view.read(cx)
                        .owner
                        .clone()
                        .update(cx, |note, cx| note.remove_view(&key, cx));
                }
                self.restored_note_views.clear();
                self.center_groups = vec![merged];
                self.active_center = 0;
                self.center_sizes.clear();
                self.center_order = self.center_groups[0].clone();
                self.restored_center = self.center_groups.clone();
                self.rebuild_center(window, cx);
                if let Some(panel) = self.panel_for(&origin, cx) {
                    self.put_center_panel(&origin, panel, window, cx);
                    self.center_order = self.center_groups[0].clone();
                    self.restored_center = self.center_groups.clone();
                    self.save_center_layout(cx);
                }
                return;
            }
            LayoutAction::ClosePane(pane) => {
                if pane >= self.center_groups.len() || !self.center_groups[pane].is_empty() {
                    return;
                }
                self.center_groups.remove(pane);
                self.active_center = 0;
            }
            LayoutAction::MoveTo { name, pane } => {
                if pane >= self.center_groups.len()
                    || crate::tabs::is_fixed(&name)
                    || !self.center_order.contains(&name)
                {
                    return;
                }
                for group in &mut self.center_groups {
                    group.retain(|n| n != &name);
                }
                self.center_groups[pane].push(name);
                self.active_center = pane;
            }
            LayoutAction::Move { from, to } => {
                if from == to || crate::tabs::is_fixed(&from) || crate::tabs::is_fixed(&to) {
                    return;
                }
                let Some((source, index)) = self
                    .center_groups
                    .iter()
                    .enumerate()
                    .find_map(|(g, n)| n.iter().position(|n| n == &from).map(|i| (g, i)))
                else {
                    return;
                };
                let Some(target) = self.center_groups.iter().position(|g| g.contains(&to)) else {
                    return;
                };
                self.center_groups[source].remove(index);
                let index = self.center_groups[target]
                    .iter()
                    .position(|n| n == &to)
                    .unwrap_or(0);
                self.center_groups[target].insert(index, from);
                self.active_center = target;
            }
            LayoutAction::Split { name, left } => {
                if crate::tabs::is_fixed(&name) {
                    return;
                }
                if self.center_groups.len() >= 4 {
                    crate::feedback::error(t(cx, "tabs.split.limit"), cx);
                    return;
                }
                let Some(i) = self.center_groups.iter().position(|g| g.contains(&name)) else {
                    return;
                };
                let owner = self
                    .note_panels
                    .get(&name)
                    .cloned()
                    .or_else(|| self.note_views.get(&name).map(|v| v.read(cx).owner.clone()));
                let split_name = if let Some(owner) = owner {
                    let key = format!("note-view-{}", uuid::Uuid::new_v4());
                    let panel = cx.new(|cx| crate::notebook::NotePane::new(key.clone(), owner, cx));
                    self.note_views.insert(key.clone(), panel);
                    key
                } else {
                    self.center_groups[i].retain(|n| n != &name);
                    name
                };
                let target = i + usize::from(!left);
                self.center_groups.insert(target, vec![split_name]);
                self.active_center = target;
            }
        }
        self.center_order = self.center_groups.iter().flatten().cloned().collect();
        self.restored_center = self.center_groups.clone();
        if old_count != self.center_groups.len() {
            self.center_sizes.clear();
        }
        self.rebuild_center(window, cx);
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SavedTab {
    key: String,
    target: crate::database::OpenTable,
    comparison: Option<[crate::database::OpenTable; 2]>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct CenterLayout {
    #[serde(default)]
    tables: Vec<SavedTab>,
    #[serde(default)]
    permissions: Vec<crate::database::PermissionBookmark>,
    #[serde(default)]
    note_views: Vec<(String, String)>,
    groups: Vec<Vec<String>>,
    sizes: Vec<f32>,
}
impl CenterLayout {
    fn valid(&self) -> bool {
        self.note_views.len() <= 48
            && self.note_views.iter().all(|(key, owner)| {
                key.starts_with("note-view-")
                    && owner.starts_with("note-")
                    && !owner.starts_with("note-view-")
                    && key != owner
                    && key.len() <= 4096
                    && owner.len() <= 4096
            })
            && self
                .note_views
                .iter()
                .map(|(key, _)| key)
                .collect::<std::collections::HashSet<_>>()
                .len()
                == self.note_views.len()
            && self.permissions.len() <= 128
            && self
                .permissions
                .iter()
                .all(|p| p.key.len() <= 4096 && p.user.len() <= 256 && p.host.len() <= 256)
            && self.tables.len() <= 128
            && self.tables.iter().all(|tab| {
                tab.key.len() <= 4096
                    && std::iter::once(&tab.target)
                        .chain(tab.comparison.iter().flat_map(|targets| targets.iter()))
                        .all(|t| {
                            t.database.len() <= 4096
                                && t.object.schema.len() <= 4096
                                && t.object.name.len() <= 4096
                        })
            })
            && self.groups.len() <= 4
            && self.groups.iter().map(Vec::len).sum::<usize>() <= 128
            && self
                .groups
                .iter()
                .flatten()
                .all(|s| s.len() <= 4096 && !crate::tabs::is_fixed(s))
            && {
                let values = self
                    .groups
                    .iter()
                    .flatten()
                    .collect::<std::collections::HashSet<_>>();
                values.len() == self.groups.iter().map(Vec::len).sum::<usize>()
            }
            && self.sizes.len() <= 4
            && self
                .sizes
                .iter()
                .all(|s| s.is_finite() && *s >= 120. && *s <= 32000.)
    }
}
impl Workspace {
    pub(super) fn restore_pending_tables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_table_restore.is_empty() {
            return;
        }
        match self.database.read(cx).profiles_status() {
            None => return,
            Some(false) => {
                self.layout_error = true;
                return;
            }
            Some(true) => {}
        }
        let pending = std::mem::take(&mut self.pending_table_restore);
        cx.defer_in(window, move |this, window, cx| {
            for saved in pending {
                if this.table_panels.contains_key(&saved.key)
                    || this
                        .database
                        .read(cx)
                        .note_profile(saved.target.connection)
                        .is_none()
                {
                    continue;
                }
                let key = saved.key.clone();
                let source = this.database.downgrade();
                let panel = cx.new(|cx| {
                    crate::table_view::TableView::restored(
                        key.clone(),
                        source,
                        saved.target,
                        saved.comparison,
                        window,
                        cx,
                    )
                });
                this.watch_table(&key, &panel, cx);
                this.table_panels.insert(key.clone(), panel);
                this.center_order.push(key.clone());
                let i = this
                    .restored_center
                    .iter()
                    .position(|g| g.contains(&key))
                    .unwrap_or(0)
                    .min(this.center_groups.len().saturating_sub(1));
                if this.center_groups.is_empty() {
                    this.center_groups.push(vec![]);
                }
                this.center_groups[i].push(key);
            }
            for (i, group) in this.center_groups.iter_mut().enumerate() {
                group.sort_by_key(|key| {
                    this.restored_center
                        .get(i)
                        .and_then(|g| g.iter().position(|n| n == key))
                        .unwrap_or(usize::MAX)
                });
            }
            this.rebuild_center(window, cx);
        });
    }
    pub(super) fn watch_table(
        &mut self,
        key: &str,
        panel: &Entity<crate::table_view::TableView>,
        cx: &mut Context<Self>,
    ) {
        if !self.table_drafts.contains_key(key) {
            let subscription = cx.subscribe(
                panel,
                |this, _, _: &crate::table_view::TableDraftChanged, cx| this.save_center_layout(cx),
            );
            self.table_drafts.insert(key.into(), subscription);
        }
    }
    pub(super) fn put_center_panel(
        &mut self,
        name: &str,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_center(name, cx);
        let active = self.center_tabs.read(cx).active_panel(cx);
        if active
            .as_ref()
            .is_some_and(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }
        let placeholder = active.filter(|p| p.panel_name(cx) == "empty-center");
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.remove_panel(panel.clone(), window, cx);
            tabs.add_panel(panel, window, cx);
            if let Some(placeholder) = placeholder {
                tabs.remove_panel(placeholder, window, cx);
            }
        });
        for group in &mut self.center_groups {
            group.retain(|n| n != name);
        }
        self.center_groups[self.active_center].push(name.into());
    }
    pub(super) fn record_center_open(&mut self, _name: &str, cx: &mut Context<Self>) {
        self.reconcile_center();
        self.save_center_layout(cx);
    }
    pub(super) fn observe_center_sizes(&mut self, source: &Entity<DockArea>, cx: &App) {
        if let gpui_component::dock::PanelInfo::Stack { sizes, .. } =
            source.read(cx).dump(cx).center.info
            && sizes.len() == self.center_groups.len()
        {
            self.center_sizes = sizes.iter().map(|v| f32::from(*v)).collect();
        }
    }
    pub(super) fn load_center_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(path) = crate::preferences::Preferences::path() else {
            return;
        };
        let path = path.with_file_name("workspace-layout.json");
        let work = cx.background_executor().spawn(async move {
            crate::app_store::load::<CenterLayout>(
                &path,
                "workspace_layout",
                1_048_576,
                CenterLayout::valid,
            )
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.layout_ready = true;
                    match result {
                        Ok(layout) => {
                            this.pending_table_restore = layout.tables;
                            this.restored_note_views = layout.note_views;
                            let notes = this
                                .note_panels
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect::<Vec<_>>();
                            for (key, note) in notes {
                                this.restore_note_views(&key, &note, window, cx);
                            }
                            for saved in layout.permissions {
                                let key = saved.key.clone();
                                let source = this.database.downgrade();
                                let panel = cx.new(|cx| {
                                    crate::database::Privileges::restored(source, saved, window, cx)
                                });
                                this.permission_panels.insert(key.clone(), panel);
                                if !this.center_order.contains(&key) {
                                    this.center_order.push(key);
                                }
                            }
                            if !layout.groups.is_empty() && this.center_groups.len() == 1 {
                                this.restored_center = layout.groups;
                                this.center_sizes = layout.sizes;
                                this.center_groups = vec![vec![]; this.restored_center.len()];
                                for name in &this.center_order {
                                    let i = this
                                        .restored_center
                                        .iter()
                                        .position(|g| g.contains(name))
                                        .unwrap_or(0);
                                    this.center_groups[i].push(name.clone());
                                }
                                this.active_center = 0;
                                this.rebuild_center(window, cx);
                            }
                        }
                        Err(_) => {
                            this.layout_error = true;
                            crate::feedback::error(t(cx, "tabs.layout_failed"), cx);
                        }
                    }
                })
            });
        })
        .detach();
    }
    pub(super) fn save_center_layout(&mut self, cx: &mut Context<Self>) {
        if !self.layout_ready || self.layout_error {
            return;
        }
        self.reconcile_center();
        let mut groups = self.restored_center.clone();
        if groups.len() != self.center_groups.len() {
            groups = self.center_groups.clone();
        } else {
            for (i, group) in self.center_groups.iter().enumerate() {
                for name in group {
                    for existing in &mut groups {
                        existing.retain(|n| n != name);
                    }
                    groups[i].push(name.clone());
                }
            }
        }
        let mut tables = self.pending_table_restore.clone();
        tables.extend(
            self.table_panels
                .iter()
                .filter(|(key, _)| self.center_order.contains(key))
                .map(|(key, panel)| {
                    let (target, comparison) = panel.read(cx).restoration(cx);
                    SavedTab {
                        key: key.clone(),
                        target,
                        comparison,
                    }
                }),
        );
        let layout = CenterLayout {
            note_views: self
                .note_views
                .iter()
                .map(|(key, view)| (key.clone(), view.read(cx).owner.read(cx).key.clone()))
                .collect(),
            tables,
            groups,
            sizes: self.center_sizes.clone(),
            permissions: self
                .permission_panels
                .values()
                .map(|p| p.read(cx).bookmark())
                .collect(),
        };
        if !layout.valid() {
            return;
        }
        let Ok(path) = crate::preferences::Preferences::path() else {
            return;
        };
        let path = path.with_file_name("workspace-layout.json");
        self.layout_task = Some(cx.spawn(async move |view, cx| {
            gpui::Timer::after(std::time::Duration::from_millis(600)).await;
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::app_store::save(&path, "workspace_layout", &layout, 1_048_576)
                })
                .await;
            if result.is_err() {
                let _ = view.update(cx, |_, cx| {
                    crate::feedback::error(t(cx, "tabs.layout_failed"), cx)
                });
            }
        }));
    }
}
impl Workspace {
    pub(super) fn restore_note_views(
        &mut self,
        key: &str,
        owner: &Entity<crate::notebook::NoteView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for (alias, _) in self
            .restored_note_views
            .clone()
            .into_iter()
            .filter(|(_, source)| source == key)
        {
            if self.note_views.contains_key(&alias) {
                continue;
            }
            let view =
                cx.new(|cx| crate::notebook::NotePane::new(alias.clone(), owner.clone(), cx));
            self.note_views.insert(alias.clone(), view);
            changed = true;
            if !self.center_order.contains(&alias) {
                self.center_order.push(alias.clone());
            }
            if let Some(i) = self.restored_center.iter().position(|g| g.contains(&alias))
                && i < self.center_groups.len()
                && !self.center_groups[i].contains(&alias)
            {
                self.center_groups[i].push(alias);
            }
        }
        if changed {
            self.rebuild_center(window, cx);
        }
    }
    pub(super) fn close_note_view(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let alias = if self.note_views.contains_key(name) {
            Some(name.to_owned())
        } else {
            self.note_views
                .iter()
                .find(|(_, view)| view.read(cx).owner.read(cx).key == name)
                .map(|(k, _)| k.clone())
        };
        let Some(alias) = alias else {
            return false;
        };
        let pane = self.note_views.remove(&alias).unwrap();
        let owner = pane.read(cx).owner.clone();
        if name != alias {
            // Preserve the remaining viewport and promote it to the document's primary tab.
            owner.update(cx, |note, cx| note.promote_view(&alias, cx));
            for group in &mut self.center_groups {
                group.retain(|key| key != name);
                for key in group {
                    if key == &alias {
                        *key = name.into();
                    }
                }
            }
            self.center_order.retain(|key| key != &alias);
        } else {
            owner.update(cx, |note, cx| note.remove_view(&alias, cx));
            for group in &mut self.center_groups {
                group.retain(|key| key != name);
            }
            self.center_order.retain(|key| key != name);
        }
        self.restored_note_views.retain(|(key, _)| key != &alias);
        self.center_groups.retain(|group| !group.is_empty());
        self.restored_center = self.center_groups.clone();
        self.rebuild_center(window, cx);
        true
    }
    fn select_note(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let matches = |name: &String| {
            name == key
                || self
                    .note_views
                    .get(name)
                    .is_some_and(|v| v.read(cx).owner.read(cx).key == key)
        };
        let target = self
            .center_groups
            .get(self.active_center)
            .and_then(|g| g.iter().find(|name| matches(name)))
            .cloned()
            .or_else(|| {
                self.center_groups
                    .iter()
                    .flatten()
                    .find(|name| matches(name))
                    .cloned()
            });
        if let Some(target) = target {
            self.activate_center(&target, cx);
            if let Some(panel) = self.panel_for(&target, cx) {
                self.center_tabs.update(cx, |tabs, cx| {
                    tabs.select_panel(panel.view().entity_id(), window, cx)
                });
            }
        }
    }
    pub(super) fn sync_open_notes(&self, cx: &mut App) {
        let mut seen = std::collections::HashSet::new();
        let mut notes = vec![];
        for name in self.center_groups.iter().flatten() {
            let owner = self
                .note_panels
                .get(name)
                .cloned()
                .or_else(|| self.note_views.get(name).map(|v| v.read(cx).owner.clone()));
            let Some(owner) = owner else {
                if !seen.insert(name.clone()) {
                    continue;
                }
                let (title, project, connection, dirty, icon) =
                    if let Some(panel) = self.table_panels.get(name) {
                        let (title, project, connection) = panel.read(cx).tab_identity(cx);
                        (
                            title,
                            project,
                            connection,
                            false,
                            IconName::GalleryVerticalEnd,
                        )
                    } else if let Some(panel) = self.permission_panels.get(name) {
                        let (title, project, connection, dirty) = panel.read(cx).tab_identity(cx);
                        (title, project, connection, dirty, IconName::User)
                    } else {
                        (
                            t(
                                cx,
                                if name == "settings" {
                                    "settings.title"
                                } else {
                                    "notes.title"
                                },
                            )
                            .to_string(),
                            String::new(),
                            String::new(),
                            false,
                            IconName::BookOpen,
                        )
                    };
                notes.push(crate::tabs::OpenNote {
                    key: name.clone(),
                    title,
                    project,
                    connection,
                    dirty,
                    icon,
                    position: String::new(),
                });
                continue;
            };
            let note = owner.read(cx);
            if !seen.insert(note.key.clone()) {
                continue;
            }
            let (title, project, connection, dirty) = note.search_identity(cx);
            let panes = self
                .center_groups
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.iter().any(|n| {
                        n == &note.key
                            || self
                                .note_views
                                .get(n)
                                .is_some_and(|v| v.read(cx).owner == owner)
                    })
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let position = if self.center_groups.len() <= 1 {
                String::new()
            } else if self.center_groups.len() == 2 && panes.len() == 2 {
                t(cx, "notes.opened.both").to_string()
            } else if self.center_groups.len() == 2 {
                t(
                    cx,
                    if panes == [0] {
                        "notes.opened.left"
                    } else {
                        "notes.opened.right"
                    },
                )
                .to_string()
            } else {
                panes
                    .iter()
                    .map(|i| (i + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            notes.push(crate::tabs::OpenNote {
                key: note.key.clone(),
                icon: IconName::BookOpen,
                title,
                project,
                connection,
                position,
                dirty,
            });
        }
        cx.global_mut::<crate::tabs::TabRouter>().2 = notes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn note_view_layout_roundtrips_without_duplicating_document_identity() {
        let layout = CenterLayout {
            groups: vec![
                vec!["note-original".into()],
                vec!["note-view-secondary".into()],
            ],
            note_views: vec![("note-view-secondary".into(), "note-original".into())],
            ..Default::default()
        };
        assert!(layout.valid());
        let saved = serde_json::to_string(&layout).unwrap();
        let mut restored: CenterLayout = serde_json::from_str(&saved).unwrap();
        assert!(restored.valid());
        restored.note_views.push(restored.note_views[0].clone());
        assert!(!restored.valid());
        restored.note_views = vec![("note-view-secondary".into(), "note-view-secondary".into())];
        assert!(!restored.valid());
    }
    #[::core::prelude::v1::test]
    fn restored_targets_never_restore_connection_generation_or_write_grants() {
        let target = crate::database::OpenTable {
            connection: uuid::Uuid::nil(),
            database: "db".into(),
            object: turbodbn_services::database::workbench::Object {
                schema: String::new(),
                name: "table".into(),
                kind: "table".into(),
                comment: String::new(),
                parent: String::new(),
            },
            mode: crate::table_view::Mode::Design,
            generation: 99,
            kind: turbodbn_services::DatabaseKind::MySql,
            writable: true,
            mysql_raw_gbk: true,
        };
        let text = serde_json::to_value(&target).unwrap();
        assert!(text.get("generation").is_none());
        assert!(text.get("writable").is_none());
        assert!(text.get("mysql_raw_gbk").is_none());
        let restored: crate::database::OpenTable = serde_json::from_value(text).unwrap();
        assert_eq!(restored.generation, 0);
        assert!(!restored.writable);
        assert!(!restored.mysql_raw_gbk);
    }
    #[::core::prelude::v1::test]
    fn stored_layout_rejects_duplicate_tabs_and_fixed_tools() {
        assert!(
            CenterLayout {
                groups: vec![vec!["note-a".into()], vec![]],
                tables: vec![],
                permissions: vec![],
                note_views: vec![],
                sizes: vec![400., 400.]
            }
            .valid()
        );
        assert!(
            !CenterLayout {
                groups: vec![vec!["note-a".into()], vec!["note-a".into()]],
                tables: vec![],
                permissions: vec![],
                note_views: vec![],
                sizes: vec![]
            }
            .valid()
        );
        assert!(
            !CenterLayout {
                groups: vec![vec!["schema".into()]],
                tables: vec![],
                permissions: vec![],
                note_views: vec![],
                sizes: vec![]
            }
            .valid()
        );
        assert!(
            !CenterLayout {
                groups: vec![vec![]; 5],
                tables: vec![],
                permissions: vec![],
                note_views: vec![],
                sizes: vec![]
            }
            .valid()
        );
    }
}
