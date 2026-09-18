use crate::{
    catalog,
    locale::{Locale, t},
    preferences::{AgentExtension, GeneralPreferences, Preferences, ProviderPreferences},
    settings::Category,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, Theme, ThemeMode,
    button::Button,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};

const PROVIDERS: [&str; 6] = ["OpenAI", "Gemini", "DeepSeek", "Qwen", "Ollama", "Custom"];
const MODES: [&str; 3] = [
    "OpenAI Chat Completions",
    "OpenAI Responses",
    "Anthropic Messages",
];

pub struct Configuration {
    pub category: Category,
    general: GeneralPreferences,
    providers: Vec<ProviderPreferences>,
    extensions: Vec<AgentExtension>,
    provider: usize,
    agent: usize,
    fields: Vec<Entity<InputState>>,
    font: Entity<InputState>,
    agent_fields: Vec<Entity<InputState>>,
    baseline: (
        GeneralPreferences,
        Vec<ProviderPreferences>,
        Vec<AgentExtension>,
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
        let font = input(&prefs.general.font_family, false, window, cx);
        let subscriptions = fields
            .iter()
            .chain(agent_fields.iter())
            .chain(std::iter::once(&font))
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
            .collect();
        Self {
            category: Category::General,
            general: prefs.general.clone(),
            baseline: (prefs.general, providers.clone(), extensions.clone()),
            providers,
            extensions,
            provider: 0,
            agent: 0,
            fields,
            font,
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
        (general, providers, extensions)
    }
    pub fn reset_theme(&mut self, cx: &mut Context<Self>) {
        self.general.theme = None;
        cx.notify();
    }
    pub fn signature(&self, cx: &App) -> String {
        serde_json::to_string(&self.snapshot(cx)).unwrap_or_default()
    }
    pub fn dirty(&self, cx: &App) -> bool {
        self.snapshot(cx) != self.baseline
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
        let (g, p, a) = self.snapshot(cx);
        self.general = g;
        self.providers = p;
        self.extensions = a;
    }
    pub fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (g, p, a) = self.baseline.clone();
        self.general = g;
        self.providers = p;
        self.extensions = a;
        self.provider = 0;
        self.agent = 0;
        self.font.update(cx, |input, cx| {
            input.set_value(self.general.font_family.clone(), window, cx)
        });
        self.sync_fields(window, cx);
        cx.notify();
    }
    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        if self.locked {
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
                    .text_size(rems(crate::typography::BODY))
                    .disabled(self.locked),
            )
    }
}
impl Render for Configuration {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().v_flex().gap_4().w_full().max_w(px(760.));
        if self.category == Category::General {
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
                .child(self.field("settings.font.family", &self.font, cx))
                .child(t(cx, "settings.font.size"))
                .child(
                    Button::new("font-size")
                        .icon(IconName::ALargeSmall)
                        .label(format!("{} px", self.general.font_size))
                        .disabled(self.locked)
                        .dropdown_caret(true)
                        .dropdown_menu({
                            let entity = cx.entity();
                            move |mut menu, _, _| {
                                for size in [12, 13, 14, 16, 18, 20, 24] {
                                    let entity = entity.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(format!("{size} px")).on_click(
                                            move |_, _, cx| {
                                                entity.update(cx, |this, cx| {
                                                    this.general.font_size = size as f32;
                                                    cx.notify();
                                                })
                                            },
                                        ),
                                    );
                                }
                                menu
                            }
                        }),
                );
        } else if self.category == Category::Providers {
            body = body
                .child(
                    div()
                        .text_size(rems(crate::typography::HEADING))
                        .child(t(cx, "settings.providers.title")),
                )
                .child(
                    Button::new("add-provider")
                        .icon(IconName::Plus)
                        .label(t(cx, "providers.action.add"))
                        .disabled(self.locked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.capture(cx);
                            this.providers.push(ProviderPreferences::default());
                            this.provider = this.providers.len() - 1;
                            this.sync_fields(window, cx);
                            cx.notify();
                        })),
                );
            if !self.providers.is_empty() {
                let entries = self
                    .providers
                    .iter()
                    .map(|p| p.display_name.clone())
                    .collect::<Vec<_>>();
                body = body.child(
                    Button::new("configured-providers")
                        .icon(IconName::Settings2)
                        .label(entries[self.provider].clone())
                        .disabled(self.locked)
                        .dropdown_caret(true)
                        .dropdown_menu({
                            let entity = cx.entity();
                            move |mut menu, _, _| {
                                for (index, name) in entries.iter().enumerate() {
                                    let entity = entity.clone();
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, window, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.capture(cx);
                                                this.provider = index;
                                                this.sync_fields(window, cx);
                                                cx.notify();
                                            })
                                        },
                                    ));
                                }
                                menu
                            }
                        }),
                );
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
                                        menu =
                                            menu.item(PopupMenuItem::new(model.clone()).on_click(
                                                move |_, window, cx| {
                                                    entity.update(cx, |this, cx| {
                                                        this.fields[3].update(cx, |input, cx| {
                                                            input.set_value(
                                                                model.clone(),
                                                                window,
                                                                cx,
                                                            )
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
                                this.providers.remove(this.provider);
                                this.provider = 0;
                                this.sync_fields(window, cx);
                                cx.notify();
                            })),
                    );
            }
        } else {
            body = body
                .child(
                    div()
                        .text_size(rems(crate::typography::HEADING))
                        .child(t(cx, "agents.extensions.title")),
                )
                .child(t(cx, "agents.extensions.description"))
                .child(
                    Button::new("add-agent")
                        .icon(IconName::Plus)
                        .label(t(cx, "agents.action.add"))
                        .disabled(self.locked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.capture(cx);
                            this.extensions.push(AgentExtension::default());
                            this.agent = this.extensions.len() - 1;
                            this.sync_fields(window, cx);
                            cx.notify();
                        })),
                );
            if !self.extensions.is_empty() {
                let entries = self
                    .extensions
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>();
                body = body.child(
                    Button::new("agents")
                        .icon(IconName::Bot)
                        .label(entries[self.agent].clone())
                        .disabled(self.locked)
                        .dropdown_caret(true)
                        .dropdown_menu({
                            let entity = cx.entity();
                            move |mut menu, _, _| {
                                for (i, name) in entries.iter().enumerate() {
                                    let entity = entity.clone();
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, window, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.capture(cx);
                                                this.agent = i;
                                                this.sync_fields(window, cx);
                                                cx.notify();
                                            })
                                        },
                                    ));
                                }
                                menu
                            }
                        }),
                );
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
                                this.extensions.remove(this.agent);
                                this.agent = 0;
                                this.sync_fields(window, cx);
                                cx.notify();
                            })),
                    );
            }
        }
        body
    }
}
