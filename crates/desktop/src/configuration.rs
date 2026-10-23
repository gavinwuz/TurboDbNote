use crate::{
    catalog,
    locale::{Locale, t},
    preferences::{
        AgentExtension, DatabasePreferences, GeneralPreferences, Preferences, ProviderPreferences,
    },
    settings::Category,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, Theme, ThemeMode, WindowExt,
    button::Button,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    select::{SearchableVec, Select, SelectEvent, SelectState},
};

const PROVIDERS: [&str; 6] = ["OpenAI", "Gemini", "DeepSeek", "Qwen", "Ollama", "Custom"];
const MODES: [&str; 3] = [
    "OpenAI Chat Completions",
    "OpenAI Responses",
    "Anthropic Messages",
];

pub struct Configuration {
    pub category: Category,
    sidebar_layout_focus: FocusHandle,
    general: GeneralPreferences,
    database: DatabasePreferences,
    timeout_fields: [Entity<InputState>; 2],
    pub database_tab: usize,
    providers: Vec<ProviderPreferences>,
    extensions: Vec<AgentExtension>,
    provider: usize,
    agent: usize,
    fields: Vec<Entity<InputState>>,
    font: Entity<InputState>,
    font_picker: Entity<SelectState<SearchableVec<String>>>,
    agent_fields: Vec<Entity<InputState>>,
    baseline: (
        GeneralPreferences,
        Vec<ProviderPreferences>,
        Vec<AgentExtension>,
        DatabasePreferences,
    ),
    _subscriptions: Vec<Subscription>,
    models: Vec<String>,
    loading: bool,
    generation: u64,
    pub status: String,
    advanced: bool,
    task: Option<Task<()>>,
    pub locked: bool,
}
fn input(value: &str, masked: bool, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(value.to_owned())
            .masked(masked)
    })
}
impl Configuration {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = cx.global::<Preferences>().clone();
        let providers = prefs.providers.clone();
        let extensions = prefs.extensions.clone();
        let provider = providers.first().cloned().unwrap_or_default();
        let agent = extensions.first().cloned().unwrap_or_default();
        let fields: Vec<_> = [
            &provider.display_name,
            &provider.endpoint,
            &provider.api_key,
            &provider.model,
            &provider.advanced,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, v)| input(v, i == 2, window, cx))
        .collect();
        let agent_fields: Vec<_> = [
            &agent.name,
            &agent.executable,
            &agent.working_directory,
            &agent.install_url,
        ]
        .into_iter()
        .map(|v| input(v, false, window, cx))
        .collect();
        let timeout_fields = [
            input(
                &prefs.database.connect_timeout_seconds.to_string(),
                false,
                window,
                cx,
            ),
            input(
                &prefs.database.execution_timeout_seconds.to_string(),
                false,
                window,
                cx,
            ),
        ];
        let font = input(&prefs.general.font_family, false, window, cx);
        let mut fonts = cx.text_system().all_font_names();
        fonts.push(".SystemUIFont".into());
        fonts.sort_unstable();
        fonts.dedup();
        let font_picker = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true)
        });
        let mut subscriptions: Vec<Subscription> = fields
            .iter()
            .chain(agent_fields.iter())
            .chain(timeout_fields.iter())
            .chain(std::iter::once(&font))
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
            .collect();
        subscriptions.push(cx.subscribe_in(
            &font_picker,
            window,
            |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(name)) = event
                    && !this.locked
                {
                    this.font
                        .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                    cx.notify();
                }
            },
        ));
        Self {
            category: Category::General,
            sidebar_layout_focus: cx.focus_handle(),
            general: prefs.general.clone(),
            baseline: (
                prefs.general,
                providers.clone(),
                extensions.clone(),
                prefs.database.clone(),
            ),
            database: prefs.database,
            timeout_fields,
            database_tab: 0,
            providers,
            extensions,
            provider: 0,
            agent: 0,
            fields,
            font,
            font_picker,
            agent_fields,
            _subscriptions: subscriptions,
            models: vec![],
            loading: false,
            generation: 0,
            status: String::new(),
            advanced: false,
            task: None,
            locked: false,
        }
    }
    fn provider_value(&self, cx: &App) -> ProviderPreferences {
        let mut p = self
            .providers
            .get(self.provider)
            .cloned()
            .unwrap_or_default();
        p.display_name = self.fields[0].read(cx).value().trim().into();
        p.endpoint = self.fields[1].read(cx).value().trim().into();
        p.api_key = self.fields[2].read(cx).value().trim().into();
        p.model = self.fields[3].read(cx).value().trim().into();
        p.advanced = self.fields[4].read(cx).value().trim().into();
        p
    }
    fn agent_value(&self, cx: &App) -> AgentExtension {
        AgentExtension {
            name: self.agent_fields[0].read(cx).value().trim().into(),
            executable: self.agent_fields[1].read(cx).value().trim().into(),
            working_directory: self.agent_fields[2].read(cx).value().trim().into(),
            install_url: self.agent_fields[3].read(cx).value().trim().into(),
        }
    }
    fn snapshot(
        &self,
        cx: &App,
    ) -> (
        GeneralPreferences,
        Vec<ProviderPreferences>,
        Vec<AgentExtension>,
        DatabasePreferences,
    ) {
        let mut general = self.general.clone();
        general.font_family = self.font.read(cx).value().trim().into();
        let mut providers = self.providers.clone();
        if let Some(p) = providers.get_mut(self.provider) {
            *p = self.provider_value(cx);
        }
        let mut extensions = self.extensions.clone();
        if let Some(a) = extensions.get_mut(self.agent) {
            *a = self.agent_value(cx);
        }
        let mut database = self.database.clone();
        database.connect_timeout_seconds = self.timeout_fields[0]
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(database.connect_timeout_seconds);
        database.execution_timeout_seconds = self.timeout_fields[1]
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(database.execution_timeout_seconds);
        (general, providers, extensions, database)
    }
    pub fn reset_theme(&mut self, cx: &mut Context<Self>) {
        self.general.theme = None;
        cx.notify();
    }
    pub fn signature(&self, cx: &App) -> String {
        serde_json::to_string(&(
            self.snapshot(cx),
            self.timeout_fields
                .iter()
                .map(|v| v.read(cx).value().to_string())
                .collect::<Vec<_>>(),
        ))
        .unwrap_or_default()
    }
    pub fn dirty(&self, cx: &App) -> bool {
        self.snapshot(cx) != self.baseline
            || self.timeout_fields[0]
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .ok()
                != Some(self.baseline.3.connect_timeout_seconds)
            || self.timeout_fields[1]
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .ok()
                != Some(self.baseline.3.execution_timeout_seconds)
    }
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = self
            .providers
            .get(self.provider)
            .cloned()
            .unwrap_or_default();
        for (field, value) in
            self.fields
                .iter()
                .zip([p.display_name, p.endpoint, p.api_key, p.model, p.advanced])
        {
            field.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        let a = self.extensions.get(self.agent).cloned().unwrap_or_default();
        for (field, value) in
            self.agent_fields
                .iter()
                .zip([a.name, a.executable, a.working_directory, a.install_url])
        {
            field.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.models.clear();
        self.generation += 1;
        self.loading = false;
        self.status.clear();
    }
    fn capture(&mut self, cx: &App) {
        let (g, p, a, d) = self.snapshot(cx);
        self.general = g;
        self.database = d;
        self.providers = p;
        self.extensions = a;
    }
    pub fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (g, p, a, d) = self.baseline.clone();
        self.general = g;
        self.database = d;
        self.providers = p;
        self.extensions = a;
        self.provider = 0;
        self.agent = 0;
        self.font.update(cx, |input, cx| {
            input.set_value(self.general.font_family.clone(), window, cx)
        });
        self.timeout_fields[0].update(cx, |field, cx| {
            field.set_value(
                self.database.connect_timeout_seconds.to_string(),
                window,
                cx,
            )
        });
        self.timeout_fields[1].update(cx, |field, cx| {
            field.set_value(
                self.database.execution_timeout_seconds.to_string(),
                window,
                cx,
            )
        });
        self.sync_fields(window, cx);
        cx.notify();
    }
    #[cfg(target_os = "windows")]
    pub fn sync_close_to_tray(&mut self, value: bool, cx: &mut Context<Self>) {
        // Only synchronize the externally persisted field; keep unrelated drafts.
        self.general.close_to_tray = value;
        self.baseline.0.close_to_tray = value;
        cx.notify();
    }
    pub fn set_sidebar_layout(
        &mut self,
        layout: crate::preferences::SidebarLayout,
        cx: &mut Context<Self>,
    ) {
        if self.locked {
            return;
        }
        self.database.sidebar_layout = layout;
        // Preview immediately; SettingsView persists the draft through its shared debounce.
        let mut prefs = cx.global::<Preferences>().clone();
        prefs.database.sidebar_layout = layout;
        cx.set_global(prefs);
        cx.refresh_windows();
        cx.notify();
    }
    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        if self.locked {
            return false;
        }
        let valid = self.timeout_fields[0]
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .ok()
            .zip(
                self.timeout_fields[1]
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u64>()
                    .ok(),
            )
            .is_some_and(|(connect, execution)| {
                turbodbn_services::timeout_policy::Defaults { connect, execution }.valid()
            });
        if !valid {
            self.status = "database.timeout.defaults_invalid".into();
            cx.notify();
            return false;
        }
        self.capture(cx);
        let result = (|| -> Result<(), String> {
            if self.general.font_family.is_empty() {
                return Err("settings.error.font_required".into());
            }
            for p in &self.providers {
                catalog::validate_provider(p)?;
            }
            for a in &self.extensions {
                if a.name.is_empty() {
                    return Err("agents.error.name_required".into());
                }
                catalog::endpoint(&a.install_url)?;
                if !a.executable.is_empty() && !std::path::Path::new(&a.executable).is_file() {
                    return Err("agents.error.executable_missing".into());
                }
                if !a.working_directory.is_empty()
                    && !std::path::Path::new(&a.working_directory).is_dir()
                {
                    return Err("agents.error.directory_missing".into());
                }
            }
            let mut prefs = cx.global::<Preferences>().clone();
            prefs.general = self.general.clone();
            prefs.database = self.database.clone();
            prefs.providers = self.providers.clone();
            prefs.extensions = self.extensions.clone();
            if let Some(name) = &self.general.theme
                && let Some(theme) = gpui_component::ThemeRegistry::global(cx)
                    .themes()
                    .get(name.as_str())
                    .cloned()
            {
                Theme::global_mut(cx).apply_config(&theme);
            }
            prefs.dark = Some(cx.theme().mode.is_dark());
            prefs.save()?;
            cx.set_global(prefs);
            cx.set_global(Locale::load(self.general.language.clone()));
            crate::locale::apply_font(cx);
            cx.refresh_windows();
            self.baseline = (
                self.general.clone(),
                self.providers.clone(),
                self.extensions.clone(),
                self.database.clone(),
            );
            Ok(())
        })();
        let success = result.is_ok();
        self.status = match result {
            Ok(()) => "common.status.saved".into(),
            Err(error) => error,
        };
        cx.notify();
        success
    }
    fn fetch(&mut self, cx: &mut Context<Self>) {
        let p = self.provider_value(cx);
        let requested = p.clone();
        self.loading = true;
        self.models.clear();
        self.status = "common.status.loading".into();
        self.generation += 1;
        let generation = self.generation;
        let work = cx
            .background_executor()
            .spawn(async move { catalog::models(&p) });
        self.task = Some(cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                this.loading = false;
                if this.provider_value(cx) != requested {
                    this.status.clear();
                    cx.notify();
                    return;
                }
                match result {
                    Ok(models) => {
                        this.status = if models.is_empty() {
                            t(cx, "providers.models.empty").to_string()
                        } else {
                            crate::locale::tr(
                                cx,
                                "chat.models.loaded",
                                &[("count", &models.len().to_string())],
                            )
                            .to_string()
                        };
                        this.models = models;
                    }
                    Err(error) => this.status = error,
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn field(&self, label: &str, entity: &Entity<InputState>, cx: &App) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(rems(crate::typography::BODY))
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, label)),
            )
            .child(
                Input::new(entity)
                    .cleanable(true)
                    .text_size(rems(crate::typography::BODY))
                    .disabled(self.locked),
            )
    }
}
impl Configuration {
    fn render_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.category {
            Category::Database => self.render_database(cx),
            Category::Shortcuts => self.render_shortcuts(cx),
            Category::General => self.render_general(window, cx),
            Category::Providers => self.render_providers(cx),
            _ => self.render_extensions(cx),
        }
    }
    fn render_database(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().v_flex().gap_4().w_full().max_w(px(760.));
        if self.database_tab == 0 {
            body = body
                .child(t(cx, "database.sidebar.title"))
                .child(
                    div()
                        .track_focus(&self.sidebar_layout_focus)
                        .h_flex()
                        .gap_3()
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            let layout = match event.keystroke.key.as_str() {
                                "left" => crate::preferences::SidebarLayout::Split,
                                "right" => crate::preferences::SidebarLayout::Tree,
                                _ => return,
                            };
                            this.set_sidebar_layout(layout, cx);
                            cx.stop_propagation();
                        }))
                        .children(
                            [
                                crate::preferences::SidebarLayout::Split,
                                crate::preferences::SidebarLayout::Tree,
                            ]
                            .into_iter()
                            .map(|layout| {
                                Button::new(layout.key())
                                    .flex_1()
                                    .h(px(148.))
                                    .selected(self.database.sidebar_layout == layout)
                                    .disabled(self.locked)
                                    .child(
                                        div()
                                            .v_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                gpui_component::Icon::default()
                                                    .path(layout.icon())
                                                    .size(px(48.)),
                                            )
                                            .child(
                                                div()
                                                    .h_flex()
                                                    .gap_2()
                                                    .when(
                                                        self.database.sidebar_layout == layout,
                                                        |el| el.child(IconName::Check),
                                                    )
                                                    .child(t(cx, layout.key())),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(t(cx, layout.hint())),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.sidebar_layout_focus.focus(window);
                                        this.set_sidebar_layout(layout, cx)
                                    }))
                            }),
                        ),
                )
                .child(t(cx, "database.sidebar.hint"));
            body = body
                .child(t(cx, "settings2.page_size"))
                .child(
                    Button::new("default-page-size")
                        .label(self.database.page_size.to_string())
                        .disabled(self.locked)
                        .dropdown_menu({
                            let view = cx.entity();
                            move |mut menu, _, _| {
                                for size in [50, 100, 200, 500, 1000] {
                                    let view = view.clone();
                                    menu =
                                        menu.item(PopupMenuItem::new(size.to_string()).on_click(
                                            move |_, _, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.database.page_size = size;
                                                    cx.notify();
                                                })
                                            },
                                        ));
                                }
                                menu
                            }
                        }),
                )
                .child(t(cx, "settings2.page_hint"));
            body = body.child(
                div()
                    .v_flex()
                    .gap_2()
                    .pt_3()
                    .child(t(cx, "database.timeout.global_title"))
                    .children(self.timeout_fields.iter().enumerate().map(|(i, field)| {
                        div()
                            .v_flex()
                            .gap_1()
                            .child(t(
                                cx,
                                if i == 0 {
                                    "database.timeout.connect"
                                } else {
                                    "database.timeout.execute"
                                },
                            ))
                            .child(Input::new(field).cleanable(true).disabled(self.locked))
                            .when(
                                !field.read(cx).value().trim().parse::<u64>().is_ok_and(|v| {
                                    if i == 0 {
                                        (1..=60).contains(&v)
                                    } else {
                                        (1..=turbodbn_core::timeouts::MAX_EXECUTION_SECONDS)
                                            .contains(&v)
                                    }
                                }),
                                |row| {
                                    row.child(
                                        div()
                                            .h_flex()
                                            .gap_2()
                                            .text_sm()
                                            .child(gpui_component::Icon::new(
                                                IconName::TriangleAlert,
                                            ))
                                            .child(t(
                                                cx,
                                                if i == 0 {
                                                    "database.timeout.connect_invalid"
                                                } else {
                                                    "database.timeout.execution_invalid"
                                                },
                                            )),
                                    )
                                },
                            )
                    }))
                    .child(t(cx, "database.timeout.global_hint")),
            );
        } else {
            body = body
                .child(
                    gpui_component::switch::Switch::new("sql-wrap")
                        .label(t(cx, "settings2.sql_wrap"))
                        .checked(self.database.sql_wrap)
                        .disabled(self.locked)
                        .on_click(cx.listener(|this, value, _, cx| {
                            this.database.sql_wrap = *value;
                            cx.notify();
                        })),
                )
                .child(t(cx, "notes.sql_scope"))
                .child(
                    Button::new("sql-execute-key")
                        .label(if self.database.sql_enter {
                            "Enter"
                        } else {
                            "Ctrl+Enter"
                        })
                        .disabled(self.locked)
                        .dropdown_caret(true)
                        .dropdown_menu({
                            let view = cx.entity();
                            move |mut menu, _, _| {
                                for (enter, label) in [(false, "Ctrl+Enter"), (true, "Enter")] {
                                    let view = view.clone();
                                    menu = menu.item(PopupMenuItem::new(label).on_click(
                                        move |_, _, cx| {
                                            view.update(cx, |this, cx| {
                                                this.database.sql_enter = enter;
                                                cx.notify();
                                            })
                                        },
                                    ));
                                }
                                menu
                            }
                        }),
                )
                .child(
                    gpui_component::switch::Switch::new("sql-delete-confirm")
                        .label(t(cx, "settings2.sql_confirm"))
                        .checked(true)
                        .disabled(true),
                );
        }

        if self.database_tab == 1 {
            body = body.child(t(cx, "settings.sql_policy_default"));
            for &(key, label, _) in turbodbn_services::database::notebook_query::COMMANDS
                .iter()
                .chain(turbodbn_services::database::notebook_query::SCRIPT_COMMANDS)
            {
                if key == "script_session" {
                    body = body.child(t(cx, "script.permissions_hint"));
                }
                body = body.child(
                    gpui_component::checkbox::Checkbox::new(key)
                        .label(if key == "script_drop_trigger" {
                            t(cx, "script.drop_trigger.permission")
                        } else if key == "script_variables" {
                            t(cx, "script.variables.permission")
                        } else {
                            label.into()
                        })
                        .checked(self.database.command_enabled(key))
                        .disabled(self.locked)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.database.set_command(key, *checked);
                            cx.notify();
                        })),
                );
            }
            body = body.child(t(cx, "settings.sql_policy_hint"));
        }
        body.into_any_element()
    }
    fn render_shortcuts(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().v_flex().gap_4().w_full().max_w(px(760.));
        for (key, label) in [
            ("Ctrl+,", "settings2.key_settings"),
            ("Ctrl+J", "settings2.key_output"),
            ("Ctrl+B", "settings2.key_sidebar"),
            ("Ctrl+S", "notes.save"),
            ("Ctrl+Enter", "notes.run"),
            ("Ctrl+C", "settings2.key_copy"),
            ("F2", "settings2.key_edit"),
        ] {
            body = body.child(
                div()
                    .h_flex()
                    .justify_between()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(t(cx, label))
                    .child(key),
            );
        }

        body.into_any_element()
    }
    fn render_general(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().v_flex().gap_4().w_full().max_w(px(760.));
        let dark = cx.theme().mode.is_dark();
        let languages = cx
            .global::<Locale>()
            .packs
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect::<Vec<_>>();
        let language = languages
            .iter()
            .find(|(id, _)| *id == self.general.language)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| self.general.language.clone());
        body = body
            .child(
                div()
                    .text_size(rems(crate::typography::HEADING))
                    .child(t(cx, "settings.general.title")),
            )
            .child(t(cx, "settings.appearance.label"))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("light")
                            .icon(IconName::Sun)
                            .label(t(cx, "settings.theme.light"))
                            .selected(!dark)
                            .disabled(self.locked)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.general.theme = None;
                                Theme::change(ThemeMode::Light, Some(window), cx);
                                crate::locale::apply_font(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("dark")
                            .icon(IconName::Moon)
                            .label(t(cx, "settings.theme.dark"))
                            .selected(dark)
                            .disabled(self.locked)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.general.theme = None;
                                Theme::change(ThemeMode::Dark, Some(window), cx);
                                crate::locale::apply_font(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Button::new("extra-themes")
                    .icon(IconName::Palette)
                    .label(t(cx, "settings.theme.additional"))
                    .disabled(self.locked)
                    .dropdown_caret(true)
                    .dropdown_menu({
                        let entity = cx.entity();
                        move |mut menu, _, cx| {
                            let themes = gpui_component::ThemeRegistry::global(cx)
                                .sorted_themes()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            for name in themes {
                                let entity = entity.clone();
                                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                    move |_, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.general.theme = Some(name.clone());
                                            cx.notify();
                                        })
                                    },
                                ));
                            }
                            menu
                        }
                    }),
            )
            .child(t(cx, "settings.language.label"))
            .child(
                Button::new("language")
                    .icon(IconName::Globe)
                    .label(language)
                    .disabled(self.locked)
                    .dropdown_caret(true)
                    .dropdown_menu({
                        let entity = cx.entity();
                        move |mut menu, _, _| {
                            for (id, name) in &languages {
                                let entity = entity.clone();
                                let id = id.clone();
                                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                    move |_, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.general.language = id.clone();
                                            cx.notify();
                                        })
                                    },
                                ));
                            }
                            menu
                        }
                    }),
            )
            .child(self.typography_controls(window, cx));
        #[cfg(target_os = "windows")]
        {
            body = body.child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        gpui_component::switch::Switch::new("close-to-tray")
                            .label(t(cx, "settings.general.close_to_tray"))
                            .checked(self.general.close_to_tray)
                            .disabled(self.locked)
                            .on_click(cx.listener(|this, value, _, cx| {
                                this.general.close_to_tray = *value;
                                cx.notify();
                            })),
                    )
                    .child(t(cx, "settings.general.close_to_tray_hint")),
            );
        }

        body = body.child(
            div()
                .border_t_1()
                .border_color(cx.theme().border)
                .pt_3()
                .v_flex()
                .gap_2()
                .child(t(cx, "notes.settings.title"))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(if self.general.note_directory.is_empty() {
                                    crate::note_library::NoteLibrary::default_directory()
                                        .display()
                                        .to_string()
                                } else {
                                    self.general.note_directory.clone()
                                }),
                        )
                        .child(
                            Button::new("note-directory")
                                .icon(IconName::FolderOpen)
                                .label(t(cx, "notes.settings.choose_directory"))
                                .disabled(self.locked)
                                .on_click(cx.listener(|_, _, window, cx| {
                                    let paths = cx.prompt_for_paths(PathPromptOptions {
                                        files: false,
                                        directories: true,
                                        multiple: false,
                                        prompt: Some(t(cx, "notes.settings.choose_directory")),
                                    });
                                    cx.spawn_in(window, async move |view, cx| {
                                        if let Ok(Ok(Some(paths))) = paths.await
                                            && let Some(path) = paths.first()
                                        {
                                            let value = path.display().to_string();
                                            let _ = view.update(cx, |this, cx| {
                                                if this.locked {
                                                    return;
                                                }
                                                this.general.note_directory = value;
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .detach();
                                })),
                        )
                        .child(
                            Button::new("reset-note-directory")
                                .icon(IconName::Undo)
                                .tooltip(t(cx, "notes.settings.reset_directory"))
                                .disabled(self.locked || self.general.note_directory.is_empty())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.general.note_directory.clear();
                                    cx.notify();
                                })),
                        ),
                )
                .child(t(cx, "notes.settings.directory_hint"))
                .child(
                    gpui_component::switch::Switch::new("note-autosave")
                        .label(t(cx, "notes.settings.autosave"))
                        .checked(self.general.note_autosave)
                        .disabled(self.locked)
                        .on_click(cx.listener(|this, value, _, cx| {
                            this.general.note_autosave = *value;
                            cx.notify();
                        })),
                )
                .child(t(cx, "notes.settings.autosave_hint")),
        );
        body.into_any_element()
    }
    fn typography_controls(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div().v_flex().gap_3().child(t(cx,"settings.font.family"))
            .child(div().h_flex().gap_3()
                .child(Input::new(&self.font).cleanable(true).disabled(self.locked).flex_1().suffix(
                    Select::new(&self.font_picker).appearance(false).placeholder("").w(px(30.)).menu_width(px(360.)).search_placeholder(t(cx,"settings2.font_search")).disabled(self.locked)
                ))
                .child(Button::new("font-size").icon(IconName::ALargeSmall).label(format!("{} px",self.general.font_size)).w(px(112.)).disabled(self.locked).dropdown_caret(true).dropdown_menu({
                    let view=cx.entity();move |mut menu,_,_| {for size in [12,13,14,16,18,20,24] {let view=view.clone();menu=menu.item(PopupMenuItem::new(format!("{size} px")).on_click(move |_,_,cx| view.update(cx,|this,cx| {this.general.font_size=size as f32;cx.notify();})));}menu}
                })))
            .child(div().p_3().border_1().border_color(cx.theme().border).rounded_md().font_family(self.font.read(cx).value().to_string()).text_size(px(self.general.font_size)).child(gpui_component::text::TextView::markdown(SharedString::from(format!("font-sql-preview-{}-{}-{}", self.font.read(cx).value(), self.general.font_size, cx.theme().mode.is_dark())), format!("```sql\nSELECT id, name FROM users WHERE status = '启用';\n{}\n```", t(cx, "settings.font_comment")), window, cx).selectable(true).style(gpui_component::text::TextViewStyle::default().code_block(StyleRefinement::default().font_family(self.font.read(cx).value().to_string()).text_size(px(self.general.font_size))))))
            .into_any_element()
    }
    fn render_providers(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().v_flex().gap_4().w_full().max_w(px(760.));
        if !self.providers.is_empty() {
            let p = self.providers[self.provider].clone();
            body=body.child(Button::new("provider-kind").icon(IconName::Globe).label(p.provider).disabled(self.locked).dropdown_caret(true).dropdown_menu({let entity=cx.entity();move |mut menu,_,_| {for name in PROVIDERS {let entity=entity.clone();menu=menu.item(PopupMenuItem::new(name).on_click(move |_,window,cx| entity.update(cx,|this,cx| {
                    this.capture(cx); let p=&mut this.providers[this.provider]; p.provider=name.into();
                    // Changing vendors deliberately clears credentials and model identity.
                    p.display_name=name.into();p.api_key.clear();p.model.clear();p.api_mode=MODES[0].into();
                    p.endpoint=match name {"Gemini"=>"https://generativelanguage.googleapis.com/v1beta/openai", "DeepSeek"=>"https://api.deepseek.com/v1", "Qwen"=>"https://dashscope.aliyuncs.com/compatible-mode/v1", "Ollama"=>"http://localhost:11434/v1", "Custom"=>"", _=>"https://api.openai.com/v1"}.into();this.sync_fields(window,cx);cx.notify();
                })));} menu}}))
                .child(self.field("providers.name.label",&self.fields[0],cx))
                .child(Button::new("api-mode").icon(IconName::Settings2).label(p.api_mode).disabled(self.locked).dropdown_caret(true).dropdown_menu({let entity=cx.entity();move |mut menu,_,_| {for mode in MODES {let entity=entity.clone();menu=menu.item(PopupMenuItem::new(mode).on_click(move |_,_,cx| entity.update(cx,|this,cx| {this.providers[this.provider].api_mode=mode.into();this.generation+=1;this.loading=false;this.models.clear();cx.notify();})));} menu}}))
                .child(self.field("providers.endpoint.label",&self.fields[1],cx)).child(self.field("providers.key.label",&self.fields[2],cx)).child(self.field("providers.model.label",&self.fields[3],cx))
                .child(Button::new("fetch-models").icon(IconName::Search).label(t(cx,"providers.models.fetch")).disabled(self.loading || self.locked).on_click(cx.listener(|this,_,_,cx| this.fetch(cx))));
            if !self.models.is_empty() {
                let models = self.models.clone();
                body = body.child(
                    Button::new("models")
                        .icon(IconName::Bot)
                        .label(t(cx, "providers.model.label"))
                        .dropdown_caret(true)
                        .dropdown_menu({
                            let entity = cx.entity();
                            move |mut menu, _, _| {
                                for model in &models {
                                    let entity = entity.clone();
                                    let model = model.clone();
                                    menu = menu.item(PopupMenuItem::new(model.clone()).on_click(
                                        move |_, window, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.fields[3].update(cx, |input, cx| {
                                                    input.set_value(model.clone(), window, cx)
                                                })
                                            })
                                        },
                                    ));
                                }
                                menu
                            }
                        }),
                );
            }
            body = body
                .child(
                    Button::new("advanced")
                        .icon(IconName::Settings2)
                        .label(t(cx, "providers.advanced.label"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.advanced = !this.advanced;
                            cx.notify();
                        })),
                )
                .when(self.advanced, |el| {
                    el.child(self.field("providers.advanced.label", &self.fields[4], cx))
                })
                .child(
                    Button::new("delete-provider")
                        .icon(IconName::Delete)
                        .label(t(cx, "common.action.delete"))
                        .disabled(self.locked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.capture(cx);
                            let index = this.provider;
                            let name = this.providers[index].display_name.clone();
                            let view = cx.entity().downgrade();
                            crate::feedback::confirm(
                                t(cx, "feedback.provider.delete"),
                                name.clone(),
                                window,
                                cx,
                                move |window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if !this.locked
                                            && this.provider == index
                                            && this
                                                .providers
                                                .get(index)
                                                .is_some_and(|p| p.display_name == name)
                                        {
                                            this.providers.remove(index);
                                            this.provider = 0;
                                            this.sync_fields(window, cx);
                                            crate::feedback::success(
                                                t(cx, "feedback.item.removed"),
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    });
                                },
                            );
                        })),
                );
        }

        body.into_any_element()
    }
    fn render_extensions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().v_flex().gap_4().w_full().max_w(px(760.));
        if !self.extensions.is_empty() {
            for (i, label) in [
                "agents.name.label",
                "agents.executable.label",
                "agents.directory.label",
                "agents.install.url",
            ]
            .into_iter()
            .enumerate()
            {
                body = body.child(self.field(label, &self.agent_fields[i], cx));
            }
            body = body
                .child(
                    Button::new("install-agent")
                        .icon(IconName::ExternalLink)
                        .label(t(cx, "agents.install.guide"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let a = this.agent_value(cx);
                            match catalog::endpoint(&a.install_url) {
                                Ok(url) => cx.open_url(url.as_str()),
                                Err(error) => {
                                    this.status = error;
                                    cx.notify();
                                }
                            }
                        })),
                )
                .child(
                    Button::new("remove-agent")
                        .icon(IconName::Delete)
                        .label(t(cx, "common.action.delete"))
                        .disabled(self.locked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.capture(cx);
                            let index = this.agent;
                            let name = this.extensions[index].name.clone();
                            let view = cx.entity().downgrade();
                            crate::feedback::confirm(
                                t(cx, "feedback.agent.delete"),
                                name.clone(),
                                window,
                                cx,
                                move |window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if !this.locked
                                            && this.agent == index
                                            && this
                                                .extensions
                                                .get(index)
                                                .is_some_and(|p| p.name == name)
                                        {
                                            this.extensions.remove(index);
                                            this.agent = 0;
                                            this.sync_fields(window, cx);
                                            crate::feedback::success(
                                                t(cx, "feedback.item.removed"),
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    });
                                },
                            );
                        })),
                );
        }

        body.into_any_element()
    }
    pub fn open_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let body = view
                .update(cx, |this, cx| this.render_form(window, cx))
                .unwrap_or_else(|_| div().into_any_element());
            dialog
                .title(t(cx, "settings2.configure"))
                .w(px(680.))
                .overlay_closable(false)
                .child(
                    div()
                        .id("settings-detail-scroll")
                        .max_h(px(480.))
                        .overflow_y_scroll()
                        .child(body),
                )
                .footer(|_, _, _, cx| {
                    vec![crate::feedback::footer_actions().child(crate::feedback::close_button(cx))]
                })
        });
    }
}
impl Render for Configuration {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !matches!(self.category, Category::Providers | Category::Extensions) {
            return self.render_form(window, cx);
        }
        let providers = self.category == Category::Providers;
        let mut body = div().v_flex().gap_3().w_full();
        let items = if providers {
            self.providers
                .iter()
                .map(|p| (p.display_name.clone(), !p.endpoint.is_empty()))
                .collect::<Vec<_>>()
        } else {
            self.extensions
                .iter()
                .map(|p| (p.name.clone(), !p.executable.is_empty()))
                .collect()
        };
        for (index, (name, configured)) in items.into_iter().enumerate() {
            body = body.child(
                div()
                    .h_flex()
                    .gap_3()
                    .p_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(if providers {
                        IconName::Globe
                    } else {
                        IconName::Bot
                    })
                    .child(div().flex_1().child(name))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t(
                                cx,
                                if configured {
                                    "settings2.registered"
                                } else {
                                    "settings2.unconfigured"
                                },
                            )),
                    )
                    .child(
                        Button::new(("configure-item", index))
                            .icon(IconName::Settings2)
                            .label(t(cx, "settings2.configure"))
                            .disabled(self.locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.capture(cx);
                                if providers {
                                    this.provider = index;
                                } else {
                                    this.agent = index;
                                }
                                this.sync_fields(window, cx);
                                this.open_details(window, cx);
                            })),
                    ),
            );
        }
        body.child(
            Button::new("add-config-item")
                .icon(IconName::Plus)
                .label(t(cx, "settings2.add"))
                .disabled(self.locked)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.capture(cx);
                    if providers {
                        this.providers.push(ProviderPreferences::default());
                        this.provider = this.providers.len() - 1;
                    } else {
                        this.extensions.push(AgentExtension::default());
                        this.agent = this.extensions.len() - 1;
                    }
                    this.sync_fields(window, cx);
                    this.open_details(window, cx);
                    cx.notify();
                })),
        )
        .into_any_element()
    }
}
