use crate::chat::{AiLog, ChatView};
use crate::panels::WorkbenchPanel;
use crate::settings::{SettingsRequest, SettingsView};
use crate::{
    catalog,
    locale::t,
    tabs::{CloseMode, CloseTabs, TabRouter},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable, StyledExt, Theme, ThemeMode, TitleBar, WindowExt,
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
    dock: Entity<DockArea>,
    ai: Entity<ChatView>,
    settings: Entity<SettingsView>,
    center_tabs: Entity<TabPanel>,
    right_tabs: Entity<TabPanel>,
    settings_open: bool,
    settings_layout: Option<[bool; 3]>,
    closing_window: bool,
    panels: Vec<Arc<dyn PanelView>>,
    center_order: Vec<&'static str>,
    right_order: Vec<&'static str>,
    pending_close: Vec<&'static str>,
    _tab_subscription: Subscription,
    update_status: String,
    checking_update: bool,
    update_task: Option<Task<()>>,
    _settings_subscription: Subscription,
    _status_observe: Subscription,
    navigation: Entity<WorkbenchPanel>,
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
        let dock = cx.new(|cx| DockArea::new("workbench", Some(2), window, cx));
        let weak = dock.downgrade();
        let notebook = cx.new(|cx| WorkbenchPanel::notebook(window, cx));
        let navigation = cx.new(|cx| {
            WorkbenchPanel::info(
                "navigation",
                "navigation.projects",
                "notebook.welcome.title",
                "projects.empty.description",
                cx,
            )
        });
        let output = cx.new(AiLog::new);
        let ai = cx.new(|cx| ChatView::new(output.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(ai.clone(), window, cx));
        let properties = cx.new(|cx| {
            WorkbenchPanel::info(
                "properties",
                "panel.inspector.title",
                "notebook.welcome.title",
                "panel.inspector.description",
                cx,
            )
        });
        let schema = cx.new(|cx| {
            WorkbenchPanel::info(
                "schema",
                "panel.schema.title",
                "panel.schema.empty",
                "panel.schema.description",
                cx,
            )
        });
        let panels: Vec<Arc<dyn PanelView>> = vec![
            Arc::new(notebook.clone()),
            Arc::new(settings.clone()),
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
            vec![Arc::new(ai.clone()), Arc::new(properties), Arc::new(schema)],
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
            dock.set_left_dock(left, Some(px(220.)), true, window, cx);
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
                        window.remove_window();
                        return;
                    }
                    let mut names = std::mem::take(&mut this.pending_close);
                    if !names.contains(&"settings") {
                        names.push("settings");
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
        let router = cx.global::<TabRouter>().0.clone();
        let tab_subscription =
            cx.subscribe_in(&router, window, |this, _, event: &CloseTabs, window, cx| {
                if crate::tabs::is_fixed(event.name) {
                    return;
                }
                if event.name == "navigation" || event.name == "ai-output" {
                    if matches!(event.mode, CloseMode::Current) {
                        let region = if event.name == "navigation" {
                            DockPlacement::Left
                        } else {
                            DockPlacement::Bottom
                        };
                        this.toggle_region(region, window, cx);
                    }
                    return;
                }
                let order = if this.center_order.contains(&event.name) {
                    &this.center_order
                } else {
                    &this.right_order
                };
                let names = crate::tabs::targets(order, event.name, event.mode);
                if names.contains(&"settings") {
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
        let status_observe = cx.observe(&ai, |_, _, cx| cx.notify());
        Self {
            panels,
            center_order: vec!["notebook"],
            right_order: vec!["ai-chat", "properties", "schema"],
            pending_close: vec![],
            _tab_subscription: tab_subscription,
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
            self.center_order.push("settings");
        }
        self.settings_open = true;
        window.focus(&self.settings.read(cx).focus_handle(cx));
        cx.notify();
    }

    pub fn prepare_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.settings.read(cx).pending(cx) {
            self.closing_window = true;
            self.open_settings(None, window, cx);
            self.settings.update(cx, |settings, cx| settings.close(cx));
            false
        } else {
            true
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
                self.panels
                    .iter()
                    .find(|p| p.panel_name(cx) == *name)
                    .cloned()
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
    fn close_names(&mut self, names: &[&'static str], window: &mut Window, cx: &mut Context<Self>) {
        self.leave_focus(window, cx);
        self.dock
            .update(cx, |dock, cx| dock.set_zoomed_out(window, cx));
        self.ai.update(cx, |chat, cx| chat.sync_zoom(false, cx));
        for name in names {
            if crate::tabs::is_fixed(name) {
                continue;
            }
            let Some(panel) = self
                .panels
                .iter()
                .find(|p| p.panel_name(cx) == *name)
                .cloned()
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
        self.center_order.retain(|n| !names.contains(n));
        self.right_order
            .retain(|n| crate::tabs::is_fixed(n) || !names.contains(n));
        self.settings_open = self.center_order.contains(&"settings");
        if names.contains(&"settings")
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
        if !self.center_order.contains(&"notebook") {
            self.center_order.push("notebook");
            self.rebuild_tabs(true, window, cx);
        }
        self.right_order = vec!["ai-chat", "properties", "schema"];
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
        self.navigation
            .update(cx, |panel, cx| panel.show_navigation(connections, cx));
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
                    .child(crate::locale::t(cx, "help.memory.description")),
            )
        });
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog_layer = gpui_component::Root::render_dialog_layer(window, cx);
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
                icon_button("project", IconName::Folder, "navigation.projects", cx)
                    .selected(left && !self.connections)
                    .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx))),
            )
            .child(
                icon_button(
                    "connections",
                    Icon::default().path("icons/database.svg"),
                    "navigation.connections",
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
                                .icon(IconName::Redo)
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
                    .child("TurboDbNote"),
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
                                            .child(t(cx, "status.database.disconnected")),
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
            .children(dialog_layer)
    }
}
