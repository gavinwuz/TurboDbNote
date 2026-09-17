use crate::chat_history::{HistoryWriter, LastChat, SavedMessage};
use crate::preferences::EnginePreferences;
use crate::{preferences::Preferences, settings::Category};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
    text::TextView,
};
use std::collections::HashMap;
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};
use turbodbn_services::agent::{self, Engine};
use turbodbn_services::codex::{ClientCommand, Connection, Event, Model, Prompt};
use turbodbn_services::codex_discovery::{self, Candidate};

const TEXT_BUDGET: usize = 1024 * 1024;
struct Message {
    item: String,
    text: String,
    user: bool,
    visible: String,
}

struct LogLine {
    time: String,
    text: String,
    error: bool,
}
pub struct AiLog {
    focus: FocusHandle,
    lines: VecDeque<LogLine>,
    scroll: ScrollHandle,
    follow: bool,
}
impl AiLog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            lines: VecDeque::new(),
            scroll: ScrollHandle::new(),
            follow: true,
        }
    }
    fn push(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        if self.lines.len() == 200 {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            text: text.chars().take(2000).collect(),
            error,
        });
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }
}
impl EventEmitter<PanelEvent> for AiLog {}
impl Focusable for AiLog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for AiLog {
    fn panel_name(&self) -> &'static str {
        "ai-output"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_flex()
            .gap_2()
            .child(IconName::SquareTerminal)
            .child(crate::locale::t(cx, "输出"))
    }
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("follow-log")
                .ghost()
                .icon(IconName::ArrowDown)
                .w(px(28.))
                .h(px(28.))
                .p_0()
                .tooltip(crate::locale::t(cx, "跟随输出"))
                .selected(self.follow)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.follow = !this.follow;
                    if this.follow {
                        this.scroll.scroll_to_bottom();
                    }
                    cx.notify();
                })),
            Button::new("clear-log")
                .ghost()
                .icon(IconName::Delete)
                .w(px(28.))
                .h(px(28.))
                .p_0()
                .tooltip(crate::locale::t(cx, "清空"))
                .disabled(self.lines.is_empty())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.lines.clear();
                    cx.notify();
                })),
            Button::new("close-output")
                .ghost()
                .icon(IconName::Close)
                .w(px(28.))
                .h(px(28.))
                .p_0()
                .tooltip(crate::locale::t(cx, "关闭输出"))
                .on_click(|_, _, cx| {
                    let events = cx.global::<crate::tabs::TabRouter>().0.clone();
                    events.update(cx, |_, cx| {
                        cx.emit(crate::tabs::CloseTabs {
                            name: "ai-output",
                            mode: crate::tabs::CloseMode::Current,
                        })
                    });
                }),
        ])
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}
impl Render for AiLog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("output-log")
            .v_flex()
            .size_full()
            .min_h_0()
            .px_3()
            .py_2()
            .track_focus(&self.focus)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(rems(crate::typography::BODY))
            .child(
                div()
                    .id("ai-log-lines")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        this.follow = false;
                        cx.notify();
                    }))
                    .when(self.lines.is_empty(), |el| {
                        el.child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::locale::t(cx, "暂无执行日志")),
                        )
                    })
                    .children(self.lines.iter().enumerate().map(|(index, line)| {
                        div()
                            .id(("log-line", index))
                            .h_flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(line.time.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(if line.error {
                                        cx.theme().danger
                                    } else {
                                        cx.theme().foreground
                                    })
                                    .child(format!(
                                        "{}{}",
                                        if line.error { "[ERROR] " } else { "" },
                                        line.text
                                    )),
                            )
                    })),
            )
    }
}

struct EngineDraft {
    preferences: EnginePreferences,
    input: String,
}

pub struct ChatView {
    automatic: crate::auto_connect::AutoConnect,
    auto_connect: bool,
    restore_last_session: bool,
    history_notice: Option<String>,
    engine: Engine,
    sessions: HashMap<Engine, LastChat>,
    drafts: HashMap<Engine, EngineDraft>,
    account_status: String,
    input_observer: Option<Subscription>,
    history: HistoryWriter,
    history_interrupted: bool,
    thread_id: Option<String>,
    session_executable: String,
    session_cwd: String,
    history_last_write: Instant,
    restored_model: Option<String>,
    restored_effort: Option<String>,
    selection_menu: Option<(Point<Pixels>, Option<FocusHandle>)>,
    settings_theme_baseline: bool,
    configured: bool,
    saving: bool,
    save_task: Option<Task<()>>,
    save_status: String,
    started: Option<Instant>,
    elapsed: Duration,
    timer: Option<Task<()>>,
    frozen: bool,
    zoomed: bool,
    stopping: bool,
    discovery_task: Option<Task<()>>,
    detecting: bool,
    candidates: Vec<Candidate>,
    discovery_status: String,
    focus: FocusHandle,
    path: Entity<InputState>,
    cwd: Entity<InputState>,
    input: Entity<InputState>,
    log: Entity<AiLog>,
    connection: Option<Connection>,
    task: Option<Task<()>>,
    models: Vec<Model>,
    selected: Option<usize>,
    effort: Option<String>,
    ready: bool,
    busy: bool,
    connecting: bool,
    status: String,
    messages: Vec<Message>,
    generation: usize,
    scroll: ScrollHandle,
    follow: bool,
}
impl ChatView {
    fn schedule_auto_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = cx.global::<Preferences>().ai.get(self.engine);
        // Only persisted configuration is eligible; never auto-run an unsaved
        // manually entered or newly discovered executable.
        let configured = self.configured
            && !saved.executable.trim().is_empty()
            && !saved.working_directory.trim().is_empty()
            && self.path.read(cx).value().trim() == saved.executable
            && self.cwd.read(cx).value().trim() == saved.working_directory;
        let enabled = self.auto_connect && saved.auto_connect;
        if !self.automatic.schedule(
            self.engine,
            enabled,
            configured,
            self.connection.is_some() || self.connecting || self.busy || self.saving,
        ) {
            return;
        }
        let engine = self.engine;
        let generation = self.generation;
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = view.update(cx, |this, cx| {
                if !this.automatic.take(engine)
                    || this.engine != engine
                    || this.generation != generation
                {
                    return;
                }
                let saved = cx.global::<Preferences>().ai.get(engine);
                if !saved.auto_connect
                    || !this.auto_connect
                    || this.connection.is_some()
                    || this.saving
                    || this.path.read(cx).value().trim() != saved.executable
                    || this.cwd.read(cx).value().trim() != saved.working_directory
                {
                    return;
                }
                this.write_log(
                    format!("自动连接 {}（仅检查账号和模型，不发送消息）", engine.name()),
                    cx,
                );
                this.connect(cx);
            });
        });
    }
    fn switch_engine(&mut self, engine: Engine, window: &mut Window, cx: &mut Context<Self>) {
        if engine == self.engine || self.busy || self.saving || self.connecting {
            return;
        }
        let snapshot = self.history_snapshot();
        self.history.save(snapshot.clone());
        self.sessions.insert(self.engine, snapshot);
        self.drafts.insert(
            self.engine,
            EngineDraft {
                preferences: EnginePreferences {
                    auto_connect: self.auto_connect,
                    restore_last_session: self.restore_last_session,
                    executable: self.path.read(cx).value().trim().to_string(),
                    working_directory: self.cwd.read(cx).value().trim().to_string(),
                    model: self
                        .selected
                        .and_then(|i| self.models.get(i))
                        .map(|m| m.model.clone())
                        .or(self.restored_model.clone()),
                    effort: self.effort.clone().or(self.restored_effort.clone()),
                },
                input: self.input.read(cx).value().to_string(),
            },
        );
        self.disconnect(cx);
        self.discovery_task = None;
        self.detecting = false;
        self.candidates.clear();
        self.engine = engine;
        let loaded = self
            .sessions
            .get(&engine)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                if cx
                    .global::<Preferences>()
                    .ai
                    .get(engine)
                    .restore_last_session
                {
                    LastChat::load(engine)
                } else {
                    Ok(LastChat {
                        engine,
                        ..Default::default()
                    })
                }
            });
        let error = loaded.as_ref().err().cloned();
        let last = loaded.unwrap_or_else(|_| LastChat {
            engine,
            ..Default::default()
        });
        let saved = self.drafts.remove(&engine).unwrap_or_else(|| EngineDraft {
            preferences: cx.global::<Preferences>().ai.get(engine).clone(),
            input: String::new(),
        });
        self.auto_connect = saved.preferences.auto_connect;
        self.restore_last_session = saved.preferences.restore_last_session;
        self.history_notice = if !last.messages.is_empty() {
            Some(format!(
                "已恢复 {} 条历史消息{}",
                last.messages.len(),
                if last.interrupted {
                    " · 上次生成未完成，不会自动重发"
                } else {
                    ""
                }
            ))
        } else {
            None
        };
        self.path.update(cx, |input, cx| {
            input.set_value(saved.preferences.executable.clone(), window, cx)
        });
        let cwd = if saved.preferences.working_directory.is_empty() {
            std::env::current_dir()
                .unwrap_or_default()
                .display()
                .to_string()
        } else {
            saved.preferences.working_directory
        };
        self.cwd
            .update(cx, |input, cx| input.set_value(cwd, window, cx));
        self.input
            .update(cx, |input, cx| input.set_value(saved.input, window, cx));
        self.configured = !cx
            .global::<Preferences>()
            .ai
            .get(engine)
            .executable
            .is_empty();
        self.thread_id = last.thread;
        self.session_executable = last.executable;
        self.session_cwd = last.cwd;
        self.restored_model = saved.preferences.model.or(last.model);
        self.restored_effort = saved.preferences.effort.or(last.effort);
        self.messages = last
            .messages
            .into_iter()
            .map(|m| Message {
                item: m.item,
                visible: m.text.clone(),
                text: m.text,
                user: m.user,
            })
            .collect();
        self.elapsed = Duration::from_secs(last.elapsed_seconds);
        self.history_interrupted = last.interrupted;
        self.selection_menu = None;
        self.account_status.clear();
        self.save_status.clear();
        self.resume_follow();
        self.status = error
            .map(|e| format!("读取 {} 历史失败：{e}", engine.name()))
            .unwrap_or_else(|| format!("已切换到 {}，请连接后继续", engine.name()));
        let mut prefs = cx.global::<Preferences>().clone();
        prefs.ai.active = engine;
        cx.set_global(prefs.clone());
        self.saving = true;
        let save = cx.background_executor().spawn(async move { prefs.save() });
        self.save_task = Some(cx.spawn(async move |view, cx| {
            let result = save.await;
            let _ = view.update(cx, |this, cx| {
                this.saving = false;
                if let Err(error) = result {
                    this.save_status = format!("引擎偏好保存失败：{error}");
                }
                cx.notify();
            });
        }));
        if self.path.read(cx).value().trim().is_empty() {
            self.detect(window, cx);
        }
        cx.notify();
    }
    fn engine_control(&self, compact: bool, cx: &Context<Self>) -> AnyElement {
        let active = self.engine;
        let view = cx.entity().downgrade();
        Button::new("engine-picker")
            .ghost()
            .icon(IconName::Bot)
            .when(!compact, |button| button.label(active.name()))
            .when(compact, |button| button.w(px(32.)).h(px(32.)).p_0())
            .tooltip(format!(
                "{} · {}",
                active.name(),
                crate::locale::t(
                    cx,
                    if self.busy {
                        "请先停止当前生成，再切换引擎"
                    } else {
                        "切换 AI 引擎"
                    },
                )
            ))
            .disabled(self.busy || self.saving || self.connecting)
            .dropdown_menu(move |mut menu, _, cx| {
                menu = menu.item(PopupMenuItem::label(crate::locale::t(cx, "AI 引擎")));
                for engine in [Engine::Codex, Engine::Claude] {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(engine.name())
                            .checked(engine == active)
                            .on_click(move |_, window, cx| {
                                let _ = view
                                    .update(cx, |this, cx| this.switch_engine(engine, window, cx));
                            }),
                    );
                }
                let view = view.clone();
                menu.item(PopupMenuItem::separator()).item(
                    PopupMenuItem::new(crate::locale::t(cx, "管理 AI 服务…")).on_click(
                        move |_, _, cx| {
                            let _ = view.update(cx, |_, cx| {
                                cx.emit(crate::settings::SettingsRequest::Open(Some(Category::Ai)))
                            });
                        },
                    ),
                )
            })
            .into_any_element()
    }
    fn history_snapshot(&self) -> LastChat {
        LastChat {
            engine: self.engine,
            version: 1,
            messages: self
                .messages
                .iter()
                .map(|m| SavedMessage {
                    item: m.item.clone(),
                    text: m.text.clone(),
                    user: m.user,
                })
                .collect(),
            thread: self.thread_id.clone(),
            executable: self.session_executable.clone(),
            cwd: self.session_cwd.clone(),
            model: self
                .selected
                .and_then(|i| self.models.get(i))
                .map(|m| m.model.clone())
                .or(self.restored_model.clone()),
            effort: self.effort.clone().or(self.restored_effort.clone()),
            interrupted: self.busy || self.history_interrupted,
            elapsed_seconds: self
                .started
                .map(|t| t.elapsed())
                .unwrap_or(self.elapsed)
                .as_secs(),
        }
    }
    fn persist_history(&mut self, force: bool, cx: &mut Context<Self>) {
        if force || self.history_last_write.elapsed() >= Duration::from_secs(1) {
            self.history.save(self.history_snapshot());
            self.history_last_write = Instant::now();
            if let Some(error) = self.history.take_error() {
                self.write_error(format!("保存聊天历史失败：{error}"), cx);
            }
        }
    }
    pub fn new(log: Entity<AiLog>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = cx.global::<Preferences>().ai.active;
        let saved = cx.global::<Preferences>().ai.get(engine).clone();
        let loaded = if saved.restore_last_session {
            LastChat::load(engine)
        } else {
            Ok(LastChat {
                engine,
                ..Default::default()
            })
        };
        let history_error = loaded.as_ref().err().cloned();
        let last = loaded.unwrap_or_default();
        let cwd = std::env::current_dir()
            .unwrap_or_default()
            .display()
            .to_string();
        let cwd = if saved.working_directory.is_empty() {
            cwd
        } else {
            saved.working_directory.clone()
        };
        let mut this = Self {
            automatic: Default::default(),
            auto_connect: saved.auto_connect,
            restore_last_session: saved.restore_last_session,
            history_notice: if !last.messages.is_empty() {
                Some(format!(
                    "已恢复 {} 条历史消息{}",
                    last.messages.len(),
                    if last.interrupted {
                        " · 上次生成未完成，不会自动重发"
                    } else {
                        ""
                    }
                ))
            } else {
                None
            },
            engine,
            sessions: HashMap::new(),
            drafts: HashMap::new(),
            account_status: String::new(),
            input_observer: None,
            history: HistoryWriter::new(),
            history_interrupted: last.interrupted,
            thread_id: last.thread,
            session_executable: last.executable,
            session_cwd: last.cwd,
            history_last_write: Instant::now(),
            restored_model: last.model,
            restored_effort: last.effort,
            selection_menu: None,
            settings_theme_baseline: cx.theme().mode.is_dark(),
            configured: !saved.executable.is_empty(),
            saving: false,
            save_task: None,
            save_status: String::new(),
            started: None,
            elapsed: Duration::from_secs(last.elapsed_seconds),
            timer: None,
            frozen: false,
            zoomed: false,
            stopping: false,
            discovery_task: None,
            detecting: false,
            candidates: vec![],
            discovery_status: String::new(),
            focus: cx.focus_handle(),
            path: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(saved.executable)
                    .placeholder("AI 引擎可执行文件完整路径")
            }),
            cwd: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(cwd)
                    .placeholder("工作目录")
            }),
            input: cx.new(|cx| {
                InputState::new(window, cx)
                    .auto_grow(3, 7)
                    .placeholder("输入问题，开始 SQL 分析…")
            }),
            log,
            connection: None,
            task: None,
            models: vec![],
            selected: None,
            effort: None,
            ready: false,
            busy: false,
            connecting: false,
            status: "配置 AI 引擎后连接；使用已有 CLI 登录状态".into(),
            messages: last
                .messages
                .into_iter()
                .map(|m| Message {
                    item: m.item,
                    visible: m.text.clone(),
                    text: m.text,
                    user: m.user,
                })
                .collect(),
            generation: 0,
            scroll: ScrollHandle::new(),
            follow: true,
        };
        this.detect(window, cx);
        this.input_observer = Some(cx.observe(&this.input, |_, _, cx| cx.notify()));
        if !this.messages.is_empty() {
            this.status = if last.interrupted {
                "已恢复上次会话（上次生成未完成）"
            } else {
                "已恢复上次会话，连接后可继续"
            }
            .into();
        }
        if let Some(error) = history_error {
            this.status = format!("无法读取上次聊天：{error}");
            this.history_notice = Some(this.status.clone());
        }
        this
    }
    fn detect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detecting || self.connection.is_some() {
            return;
        }
        self.detecting = true;
        self.discovery_status = "正在检测 AI 引擎…".into();
        let before = self.path.read(cx).value().to_string();
        let engine = self.engine;
        let generation = self.generation;
        let scan = cx.background_executor().spawn(async move {
            match engine {
                Engine::Codex => codex_discovery::discover(),
                Engine::Claude => codex_discovery::discover_claude(),
            }
        });
        self.discovery_task = Some(cx.spawn_in(window, async move |view, cx| {
            let candidates = scan.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.engine != engine || this.generation != generation {
                        return;
                    }
                    this.detecting = false;
                    this.candidates = candidates;
                    if this.connection.is_some() {
                        cx.notify();
                        return;
                    }
                    this.discovery_status = match this.candidates.len() {
                        0 => "未找到可用引擎，请手动填写实际可执行文件路径".into(),
                        1 => format!(
                            "已检测：{} · {}",
                            this.candidates[0].source, this.candidates[0].version
                        ),
                        count => format!("找到 {count} 个候选，请选择版本"),
                    };
                    if this.candidates.len() == 1
                        && before.trim().is_empty()
                        && this.path.read(cx).value().as_ref() == before
                    {
                        let path = this.candidates[0].path.display().to_string();
                        this.path
                            .update(cx, |input, cx| input.set_value(path, window, cx));
                    }
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }
    fn write_log(&mut self, text: String, cx: &mut Context<Self>) {
        self.log.update(cx, |log, cx| log.push(text, false, cx));
    }
    fn write_error(&mut self, text: String, cx: &mut Context<Self>) {
        self.log.update(cx, |log, cx| log.push(text, true, cx));
    }
    pub fn connection_status(&self) -> &'static str {
        if self.busy {
            "生成中"
        } else if self.connecting {
            "连接中"
        } else if self.ready {
            "已连接"
        } else {
            "未连接"
        }
    }
    fn browse_codex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("选择实际 {}", self.engine.executable_name()).into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = selection.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.connection.is_some() {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(paths))) => {
                            if let Some(path) = paths.first() {
                                if cfg!(windows)
                                    && !path
                                        .extension()
                                        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
                                {
                                    this.discovery_status =
                                        "请选择实际 .exe，不能选择 .cmd / .ps1".into();
                                } else {
                                    this.path.update(cx, |input, cx| {
                                        input.set_value(path.display().to_string(), window, cx)
                                    });
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        _ => this.discovery_status = "无法打开文件选择窗口，请手动输入路径".into(),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.automatic.suppress(self.engine);
        self.history_interrupted |= self.busy;
        if !self.messages.is_empty() {
            self.persist_history(true, cx);
        }
        self.finish_timing();
        self.generation += 1;
        self.task = None;
        self.connection = None;
        self.ready = false;
        self.busy = false;
        self.connecting = false;
        self.models.clear();
        self.selected = None;
        self.effort = None;
        self.status = "已断开，重新连接会创建新会话".into();
        cx.notify();
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        self.discovery_task = None;
        self.detecting = false;
        self.disconnect(cx);
        self.connecting = true;
        self.status = "正在连接 AI 引擎…".into();
        let (connection, events) = agent::connect(
            self.engine,
            PathBuf::from(self.path.read(cx).value().trim()),
            PathBuf::from(self.cwd.read(cx).value().trim()),
        );
        let executable = self.path.read(cx).value().trim().to_string();
        let cwd = self.cwd.read(cx).value().trim().to_string();
        if let Some(id) = &self.thread_id {
            if self.session_executable == executable && self.session_cwd == cwd {
                let _ = connection.send(ClientCommand::RestoreThread(id.clone()));
            } else {
                self.status = "工作目录或引擎路径已更改，旧记录保留；新请求将使用新上下文".into();
                self.thread_id = None;
            }
        }
        self.session_executable = executable;
        self.session_cwd = cwd;
        self.connection = Some(connection);
        let generation = self.generation;
        self.task = Some(cx.spawn(async move |view, cx| {
            while let Ok(first) = events.recv().await {
                // Coalesce notifications at ~30Hz instead of rendering each token.
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let mut batch = vec![first];
                while batch.len() < 128 {
                    match events.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let result = view.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    for event in batch {
                        this.event(event, cx);
                    }
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }
    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(model) = self.models.get(index) {
            self.selected = Some(index);
            self.restored_model = Some(model.model.clone());
            self.effort = model
                .supported_reasoning_efforts
                .iter()
                .find(|e| e.reasoning_effort == model.default_reasoning_effort)
                .map(|e| e.reasoning_effort.clone());
            self.restored_effort = self.effort.clone();
        }
        cx.notify();
    }
    fn event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Thread(id) => {
                self.thread_id = Some(id);
                self.persist_history(true, cx);
            }
            Event::Models(models) => {
                self.models = models;
                if !self.models.is_empty() {
                    let mut saved = cx.global::<Preferences>().ai.get(self.engine).clone();
                    if self.restored_model.is_some() {
                        saved.model = self.restored_model.take();
                        saved.effort = self.restored_effort.take();
                    }
                    let index = self
                        .models
                        .iter()
                        .position(|m| Some(&m.model) == saved.model.as_ref())
                        .or_else(|| self.models.iter().position(|m| m.is_default))
                        .unwrap_or(0);
                    self.choose(index, cx);
                    if let Some(effort) = saved.effort
                        && self.models[index]
                            .supported_reasoning_efforts
                            .iter()
                            .any(|e| e.reasoning_effort == effort)
                    {
                        self.effort = Some(effort);
                    }
                }
                self.connecting = false;
                self.write_log(format!("已加载 {} 个可用模型", self.models.len()), cx);
            }
            Event::Account { ready, label } => {
                self.ready = ready;
                self.account_status = if ready { label.clone() } else { String::new() };
                self.status = label.clone();
                if ready {
                    self.write_log(label, cx);
                } else {
                    self.write_error(label, cx);
                }
            }
            Event::Log(line) => self.write_log(line, cx),
            Event::Delta { item, text } => self.apply_text(item, text, false, cx),
            Event::Snapshot { item, text } => self.apply_text(item, text, true, cx),
            Event::Completed(status) => {
                let failed = !matches!(status.as_str(), "completed" | "interrupted" | "已停止");
                self.history_interrupted = false;
                self.finish_timing();
                self.busy = false;
                self.status = match status.as_str() {
                    "completed" => "回答完成",
                    "interrupted" | "已停止" => "已停止",
                    _ => "请求未成功完成",
                }
                .into();
                if failed {
                    self.write_error(self.status.clone(), cx);
                } else {
                    self.write_log(self.status.clone(), cx);
                }
                self.persist_history(true, cx);
            }
            Event::Error(error) => {
                self.finish_timing();
                self.busy = false;
                self.connecting = false;
                self.status = error.clone();
                self.write_error(error, cx);
                self.persist_history(true, cx);
            }
            Event::Disconnected => {
                self.finish_timing();
                self.connection = None;
                self.ready = false;
                self.busy = false;
                self.connecting = false;
                self.write_log("后台连接已结束".into(), cx);
                self.persist_history(true, cx);
            }
        }
    }
    fn apply_text(&mut self, item: String, text: String, snapshot: bool, cx: &mut Context<Self>) {
        let existing = self.messages.iter().position(|m| !m.user && m.item == item);
        let total: usize = self.messages.iter().map(|m| m.text.len()).sum();
        let old = existing.map(|i| self.messages[i].text.len()).unwrap_or(0);
        let added = if snapshot {
            total.saturating_sub(old) + text.len()
        } else {
            total + text.len()
        };
        if added > TEXT_BUDGET || (existing.is_none() && self.messages.len() >= 100) {
            self.disconnect(cx);
            self.status = "会话显示达到上限，已停止；请新建会话".into();
            return;
        }
        if let Some(index) = existing {
            if snapshot {
                self.messages[index].text = text;
            } else {
                self.messages[index].text.push_str(&text);
            }
        } else {
            self.messages.push(Message {
                item,
                text,
                user: false,
                visible: String::new(),
            });
        }
        if !self.frozen {
            let index = existing.unwrap_or(self.messages.len() - 1);
            let message = &mut self.messages[index];
            message.visible.clone_from(&message.text);
        }
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        self.persist_history(false, cx);
    }
    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() || self.busy || !self.ready {
            return;
        }
        if text.len() > 65536
            || self.messages.len() >= 98
            || self.messages.iter().map(|m| m.text.len()).sum::<usize>() + text.len() > TEXT_BUDGET
        {
            self.status = "输入或会话过长，请缩短输入或新建会话".into();
            cx.notify();
            return;
        }
        let Some(model) = self.selected.and_then(|i| self.models.get(i)) else {
            return;
        };
        let prompt = Prompt {
            text: text.clone(),
            model: model.model.clone(),
            effort: self.effort.clone(),
        };
        if let Some(connection) = &self.connection {
            match connection.send(ClientCommand::Send(prompt)) {
                Ok(()) => {
                    self.messages.push(Message {
                        item: format!("user-{}", self.messages.len()),
                        visible: text.clone(),
                        text,
                        user: true,
                    });
                    self.busy = true;
                    self.history_notice = None;
                    self.history_interrupted = false;
                    self.resume_follow();
                    self.start_timing(cx);
                    self.persist_history(true, cx);
                    self.status = "正在分析…".into();
                    self.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
                Err(error) => self.status = error,
            }
        }
        cx.notify();
    }
}
impl EventEmitter<PanelEvent> for ChatView {}
impl Focusable for ChatView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChatView {
    fn panel_name(&self) -> &'static str {
        "ai-chat"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tabs::title(
            "ai-chat",
            IconName::Bot,
            crate::locale::t(cx, "AI 对话"),
            cx,
        )
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn set_zoomed(&mut self, zoomed: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }
}

fn duration_label(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

impl ChatView {
    pub fn settings_engine(&self) -> Engine {
        self.engine
    }
    pub fn settings_can_switch(&self, cx: &App) -> bool {
        !self.busy && !self.saving && !self.connecting && !self.settings_dirty(cx)
    }
    pub fn select_settings_engine(
        &mut self,
        engine: Engine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_can_switch(cx) {
            self.switch_engine(engine, window, cx);
        }
    }
    pub fn settings_saving(&self) -> bool {
        self.saving
    }
    pub fn sync_zoom(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }
    pub fn settings_inputs(&self) -> [Entity<InputState>; 2] {
        [self.path.clone(), self.cwd.clone()]
    }
    pub fn settings_busy(&self) -> bool {
        self.busy
    }
    pub fn settings_dirty(&self, cx: &App) -> bool {
        let prefs = cx.global::<Preferences>();
        let model = self
            .selected
            .and_then(|i| self.models.get(i))
            .map(|m| m.model.as_str());
        cx.theme().mode.is_dark() != prefs.dark.unwrap_or(self.settings_theme_baseline)
            || self.auto_connect != prefs.ai.get(self.engine).auto_connect
            || self.restore_last_session != prefs.ai.get(self.engine).restore_last_session
            || self.path.read(cx).value().trim() != prefs.ai.get(self.engine).executable
            || (!prefs.ai.get(self.engine).executable.is_empty()
                && self.cwd.read(cx).value().trim() != prefs.ai.get(self.engine).working_directory)
            || (self.ready
                && (model != prefs.ai.get(self.engine).model.as_deref()
                    || self.effort != prefs.ai.get(self.engine).effort))
    }
    pub fn settings_signature(&self, cx: &App) -> String {
        format!(
            "{:?}",
            (
                self.engine,
                self.path.read(cx).value(),
                self.cwd.read(cx).value(),
                self.selected,
                &self.effort,
                self.auto_connect,
                self.restore_last_session,
                cx.theme().mode.is_dark()
            )
        )
    }
    pub fn save_all_settings(&mut self, cx: &mut Context<Self>) {
        self.persist_settings(true, false, cx);
    }
    pub fn discard_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = cx.global::<Preferences>().clone();
        self.auto_connect = prefs.ai.get(self.engine).auto_connect;
        self.restore_last_session = prefs.ai.get(self.engine).restore_last_session;
        gpui_component::Theme::change(
            if prefs.dark.unwrap_or(self.settings_theme_baseline) {
                gpui_component::ThemeMode::Dark
            } else {
                gpui_component::ThemeMode::Light
            },
            Some(window),
            cx,
        );
        self.path.update(cx, |input, cx| {
            input.set_value(prefs.ai.get(self.engine).executable.clone(), window, cx)
        });
        self.cwd.update(cx, |input, cx| {
            input.set_value(
                prefs.ai.get(self.engine).working_directory.clone(),
                window,
                cx,
            )
        });
        if let Some(index) = self
            .models
            .iter()
            .position(|model| Some(&model.model) == prefs.ai.get(self.engine).model.as_ref())
        {
            self.selected = Some(index);
        }
        self.effort = prefs.ai.get(self.engine).effort.clone();
        cx.notify();
    }
    fn start_timing(&mut self, cx: &mut Context<Self>) {
        self.started = Some(Instant::now());
        self.elapsed = Duration::ZERO;
        self.stopping = false;
        self.timer = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = view
                    .update(cx, |this, cx| {
                        if !this.busy {
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }
    fn finish_timing(&mut self) {
        if let Some(started) = self.started.take() {
            self.elapsed = started.elapsed();
        }
        self.timer = None;
        self.stopping = false;
    }
    fn resume_follow(&mut self) {
        self.frozen = false;
        self.follow = true;
        for message in &mut self.messages {
            message.visible.clone_from(&message.text);
        }
        self.scroll.scroll_to_bottom();
    }
    pub fn settings_status(&self) -> String {
        self.save_status.clone()
    }
    pub fn save_settings(&mut self, ai: bool, cx: &mut Context<Self>) {
        self.persist_settings(ai, true, cx);
    }
    fn persist_settings(&mut self, ai: bool, connect_after: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        if ai && self.busy && connect_after {
            self.save_status = "请先停止当前回答，再修改 AI 设置".into();
            cx.notify();
            return;
        }
        let mut prefs = cx.global::<Preferences>().clone();
        prefs.dark = Some(cx.theme().mode.is_dark());
        if ai {
            let model = self
                .selected
                .and_then(|i| self.models.get(i))
                .map(|m| m.model.clone())
                .or_else(|| {
                    if !self.ready {
                        prefs.ai.get(self.engine).model.clone()
                    } else {
                        None
                    }
                });
            let entry = prefs.ai.get_mut(self.engine);
            entry.executable = self.path.read(cx).value().trim().to_string();
            entry.working_directory = self.cwd.read(cx).value().trim().to_string();
            entry.model = model;
            if self.ready {
                entry.effort = self.effort.clone();
            }
            entry.auto_connect = self.auto_connect;
            entry.restore_last_session = self.restore_last_session;
            prefs.ai.active = self.engine;
        }
        let engine = self.engine;
        if !connect_after {
            self.automatic.suppress(engine);
        }
        self.saving = true;
        self.save_status = "正在保存…".into();
        let saved = prefs.clone();
        let work = cx.background_executor().spawn(async move {
            if ai && (!saved.ai.get(engine).executable.is_empty() || connect_after) {
                let exe = PathBuf::from(&saved.ai.get(engine).executable);
                let cwd = PathBuf::from(&saved.ai.get(engine).working_directory);
                if !exe.is_absolute() || !exe.is_file() {
                    return Err("请选择有效的 AI 引擎可执行文件".into());
                }
                if cfg!(windows)
                    && !exe
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
                {
                    return Err("请选择实际引擎 .exe，不支持 .cmd / .ps1".into());
                }
                if !cwd.is_absolute() || !cwd.is_dir() {
                    return Err("请选择有效的工作目录".into());
                }
            }
            saved.save()
        });
        self.save_task = Some(cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        cx.set_global(prefs);
                        this.save_status = "设置已保存".into();
                        if ai {
                            this.configured = !cx
                                .global::<Preferences>()
                                .ai
                                .get(engine)
                                .executable
                                .is_empty();
                            if connect_after && this.connection.is_none() {
                                this.connect(cx);
                            }
                        }
                    }
                    Err(error) => this.save_status = format!("保存失败：{error}"),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn model_controls(&self, cx: &Context<Self>) -> AnyElement {
        let models = self.models.clone();
        let selected = self.selected;
        let view = cx.entity().downgrade();
        let label = selected
            .and_then(|i| models.get(i))
            .map(|m| m.display_name.clone())
            .unwrap_or("选择模型".into());
        let efforts = selected
            .and_then(|i| models.get(i))
            .map(|m| m.supported_reasoning_efforts.clone())
            .unwrap_or_default();
        let current = self.effort.clone();
        let effort_view = cx.entity().downgrade();
        div()
            .h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                Button::new("model")
                    .ghost()
                    .label(label)
                    .icon(IconName::ChevronDown)
                    .disabled(self.busy || models.is_empty())
                    .dropdown_menu(move |mut menu, _, _| {
                        for (index, model) in models.iter().enumerate() {
                            let view = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(model.display_name.clone())
                                    .checked(selected == Some(index))
                                    .on_click(move |_, _, cx| {
                                        let _ = view.update(cx, |this, cx| this.choose(index, cx));
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new("effort")
                    .ghost()
                    .label(current.clone().unwrap_or("默认".into()))
                    .icon(IconName::ChevronDown)
                    .disabled(self.busy || efforts.is_empty())
                    .dropdown_menu(move |mut menu, _, _| {
                        for effort in &efforts {
                            let value = effort.reasoning_effort.clone();
                            let view = effort_view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(value.clone())
                                    .checked(current.as_ref() == Some(&value))
                                    .on_click(move |_, _, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            if !this.busy {
                                                this.effort = Some(value.clone());
                                                cx.notify();
                                            }
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
    fn composer_model(&self, cx: &Context<Self>) -> AnyElement {
        let models = self.models.clone();
        let selected = self.selected;
        let current = self.effort.clone();
        let label = selected
            .and_then(|i| models.get(i))
            .map(|m| {
                format!(
                    "{} · {}",
                    m.display_name,
                    current.as_deref().unwrap_or("默认")
                )
            })
            .unwrap_or_else(|| crate::locale::t(cx, "选择模型").to_string());
        let view = cx.entity().downgrade();
        Button::new("composer-model")
            .ghost()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .tooltip(label.clone())
            .child(div().min_w_0().max_w(rems(8.)).truncate().child(label))
            .icon(IconName::ChevronDown)
            .disabled(self.busy || models.is_empty())
            .dropdown_menu(move |mut menu, _, cx| {
                menu = menu.item(PopupMenuItem::label(crate::locale::t(cx, "模型")));
                for (index, model) in models.iter().enumerate() {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(model.display_name.clone())
                            .checked(selected == Some(index))
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| this.choose(index, cx));
                            }),
                    );
                }
                if let Some(model) = selected.and_then(|i| models.get(i)) {
                    menu = menu
                        .item(PopupMenuItem::separator())
                        .item(PopupMenuItem::label(crate::locale::t(cx, "推理强度")));
                    for effort in &model.supported_reasoning_efforts {
                        let value = effort.reasoning_effort.clone();
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::new(value.clone())
                                .checked(current.as_ref() == Some(&value))
                                .on_click(move |_, _, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if !this.busy {
                                            this.effort = Some(value.clone());
                                            cx.notify();
                                        }
                                    });
                                }),
                        );
                    }
                }
                menu
            })
            .into_any_element()
    }
    fn composer(&self, cx: &Context<Self>) -> AnyElement {
        let suggestion_view = cx.entity().downgrade();
        let can_send =
            !self.input.read(cx).value().trim().is_empty() && self.ready && self.selected.is_some();
        let directory = PathBuf::from(self.cwd.read(cx).value().to_string());
        let directory = directory
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or("工作目录".into());
        div()
            .v_flex()
            .gap_2()
            .px_3()
            .pb_3()
            .flex_shrink_0()
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Button::new("chat-connection")
                            .ghost()
                            .icon(IconName::Globe)
                            .flex_1()
                            .min_w_0()
                            .px_1()
                            .child(
                                div()
                                    .max_w(rems(3.))
                                    .truncate()
                                    .child(crate::locale::t(cx, "连接")),
                            )
                            .dropdown_caret(true)
                            .tooltip(crate::locale::t(cx, "数据库连接功能尚未接入"))
                            .disabled(true),
                    )
                    .child(
                        Button::new("chat-database")
                            .ghost()
                            .icon(Icon::default().path("icons/database.svg"))
                            .flex_1()
                            .min_w_0()
                            .px_1()
                            .child(
                                div()
                                    .max_w(rems(3.))
                                    .truncate()
                                    .child(crate::locale::t(cx, "数据库")),
                            )
                            .dropdown_caret(true)
                            .tooltip(crate::locale::t(cx, "请先选择连接"))
                            .disabled(true),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted)
                    .child(
                        Input::new(&self.input)
                            .text_size(rems(crate::typography::BODY))
                            .appearance(false)
                            .min_h(rems(5.))
                            .max_h(rems(10.)),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .min_w_0()
                            .w_full()
                            .child(
                                Button::new("quick-prompts")
                                    .ghost()
                                    .w(px(32.))
                                    .h(px(32.))
                                    .icon(IconName::Plus)
                                    .tooltip(crate::locale::t(cx, "快捷提问"))
                                    .disabled(self.busy)
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for (label, text) in [
                                            ("解释 SQL", "请解释以下 SQL 的逻辑与注意事项：\n\n"),
                                            (
                                                "优化 SQL",
                                                "请分析以下 SQL 的性能，并给出优化建议：\n\n",
                                            ),
                                            ("排查错误", "请帮助排查这个数据库错误：\n\n"),
                                        ] {
                                            let view = suggestion_view.clone();
                                            menu = menu.item(PopupMenuItem::new(label).on_click(
                                                move |_, window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        if !this.busy {
                                                            let old = this
                                                                .input
                                                                .read(cx)
                                                                .value()
                                                                .to_string();
                                                            this.input.update(cx, |input, cx| {
                                                                input.set_value(
                                                                    format!("{text}{old}"),
                                                                    window,
                                                                    cx,
                                                                )
                                                            });
                                                            this.input
                                                                .read(cx)
                                                                .focus_handle(cx)
                                                                .focus(window);
                                                            cx.notify();
                                                        }
                                                    });
                                                },
                                            ));
                                        }
                                        menu
                                    }),
                            )
                            .child(
                                Button::new("read-only-mode")
                                    .ghost()
                                    .w(px(32.))
                                    .h(px(32.))
                                    .icon(IconName::Eye)
                                    .tooltip(crate::locale::t(
                                        cx,
                                        "当前仅开放只读分析，不执行数据库写入",
                                    )),
                            )
                            .child(self.engine_control(true, cx))
                            .child(div().flex_1().min_w_0().child(self.composer_model(cx)))
                            .child(
                                Button::new("send-or-stop")
                                    .primary()
                                    .rounded(px(16.))
                                    .h(px(32.))
                                    .w(px(32.))
                                    .icon(if self.busy {
                                        Icon::default().path("icons/stop.svg")
                                    } else {
                                        Icon::new(IconName::ArrowUp)
                                    })
                                    .tooltip(crate::locale::t(
                                        cx,
                                        if self.stopping {
                                            "正在停止…"
                                        } else if self.busy {
                                            "停止生成"
                                        } else {
                                            "发送"
                                        },
                                    ))
                                    .disabled(self.stopping || (!self.busy && !can_send))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.busy {
                                            if let Some(connection) = &this.connection {
                                                match connection.send(ClientCommand::Interrupt) {
                                                    Ok(()) => {
                                                        this.stopping = true;
                                                        this.status = "正在停止".into();
                                                    }
                                                    Err(error) => this.status = error,
                                                }
                                                cx.notify();
                                            }
                                        } else {
                                            this.send_prompt(window, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .gap_2()
                    .text_size(rems(crate::typography::META))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(IconName::Folder)
                            .child(div().flex_1().min_w_0().truncate().child(directory)),
                    )
                    .child(crate::locale::t(
                        cx,
                        if self.busy {
                            "正在工作"
                        } else if self.ready {
                            "已连接 · 本地"
                        } else {
                            "未连接"
                        },
                    )),
            )
            .into_any_element()
    }
    pub fn render_ai_settings(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let candidates = self.candidates.clone();
        let view = cx.entity().downgrade();
        div()
            .v_flex()
            .gap_3()
            .child(div().text_size(rems(crate::typography::HEADING)).child(crate::locale::t(cx,"AI 服务")))
            .child(Switch::new("auto-connect").label(crate::locale::t(cx,"打开 AI 面板时自动连接")).checked(self.auto_connect)
                .disabled(self.saving).on_click(cx.listener(|this,checked:&bool,_,cx|{this.auto_connect = *checked;cx.notify();})))
            .child(Switch::new("restore-session").label(crate::locale::t(cx,"启动时恢复上次会话")).checked(self.restore_last_session)
                .tooltip(crate::locale::t(cx,"下次启动生效；关闭此项不会删除本地历史"))
                .disabled(self.saving).on_click(cx.listener(|this,checked:&bool,_,cx|{this.restore_last_session = *checked;cx.notify();})))
            .when(self.engine==Engine::Claude,|el|el.child(div().text_size(rems(crate::typography::META)).text_color(cx.theme().muted_foreground)
                .child(crate::locale::t(cx,"Claude 使用 CLI 默认模型或官方别名，实际可用性由账号/服务配置决定；本版不覆盖推理强度。"))))
            .child(
                div()
                    .text_size(rems(crate::typography::META))
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::locale::t(cx,"Codex / Claude 分别保存配置与会话。切换引擎不会自动发送请求。")),
            )
            .child(crate::locale::t(cx,"可执行文件"))
            .child(Input::new(&self.path).text_size(rems(crate::typography::BODY)).disabled(self.connection.is_some() || self.saving))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("browse").icon(IconName::FolderOpen)
                            .label(crate::locale::t(cx,"浏览文件"))
                            .disabled(self.connection.is_some() || self.saving)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.browse_codex(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("detect").icon(IconName::Search)
                            .label(crate::locale::t(cx,if self.detecting {
                                "检测中…"
                            } else {
                                "重新检测"
                            }))
                            .disabled(self.detecting || self.connection.is_some() || self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.detect(window, cx))),
                    )
                    .child(
                        Button::new("candidates").icon(IconName::ChevronDown)
                            .label(crate::locale::t(cx,"检测结果"))
                            .disabled(
                                candidates.is_empty() || self.connection.is_some() || self.saving,
                            )
                            .dropdown_menu(move |mut menu, _, _| {
                                for candidate in &candidates {
                                    let value = candidate.clone();
                                    let view = view.clone();
                                    menu =
                                        menu.item(
                                            PopupMenuItem::new(format!(
                                                "{} · {} · {}",
                                                value.source,
                                                value.version,
                                                value.path.display()
                                            ))
                                            .on_click(move |_, window, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    if this.connection.is_none() {
                                                        this.path.update(cx, |input, cx| {
                                                            input.set_value(
                                                                value.path.display().to_string(),
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                        cx.notify();
                                                    }
                                                });
                                            }),
                                        );
                                }
                                menu
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(rems(crate::typography::META))
                    .text_color(cx.theme().muted_foreground)
                    .child(self.discovery_status.clone()),
            )
            .child(crate::locale::t(cx,"工作目录"))
            .child(Input::new(&self.cwd).text_size(rems(crate::typography::BODY)).disabled(self.connection.is_some() || self.saving))
            .child(
                div()
                    .text_size(rems(crate::typography::META))
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::locale::t(cx,"当前仅提供文本/只读分析；使用所选 CLI 的已有登录状态。")),
            )
            .when(!self.models.is_empty(), |el| {
                el.child(crate::locale::t(cx,"默认模型与推理强度"))
                    .child(self.model_controls(cx))
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("connect-ai")
                            .icon(IconName::Bot)
                            .primary()
                            .label(crate::locale::t(cx,"连接"))
                            .disabled(self.saving || self.busy || self.connection.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.save_settings(true, cx))),
                    )
                    .when(self.connection.is_some(), |el| {
                        el.child(
                            Button::new("disconnect-ai").icon(IconName::Close)
                                .label(crate::locale::t(cx,"断开连接"))
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                        )
                    }),
            )
            .child(div().text_size(rems(crate::typography::BODY)).child(crate::locale::t(cx,&self.status)))
            .child(
                div()
                    .text_size(rems(crate::typography::BODY))
                    .text_color(cx.theme().muted_foreground)
                    .child(self.save_status.clone()),
            )
            .into_any_element()
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_auto_connect(window, cx);
        if self.follow && !self.frozen {
            self.scroll.scroll_to_bottom();
        }
        let shell = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .text_size(rems(crate::typography::BODY))
            .bg(cx.theme().background)
            .track_focus(&self.focus);
        if !self.configured && self.messages.is_empty() {
            return shell
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .w_full()
                        .min_h_0()
                        .child(self.engine_control(false, cx))
                        .items_center()
                        .justify_center()
                        .gap_4()
                        .p_6()
                        .child(IconName::Bot)
                        .child(
                            div()
                                .text_size(rems(crate::typography::HEADING))
                                .child(crate::locale::t(cx, "开始你的 SQL 分析")),
                        )
                        .child(div().text_color(cx.theme().muted_foreground).child(
                            crate::locale::t(
                                cx,
                                "配置 AI 服务，解释 SQL、分析问题并整理查询思路。",
                            ),
                        ))
                        .child(
                            Button::new("configure-ai")
                                .primary()
                                .icon(IconName::Settings)
                                .label(crate::locale::t(cx, "前往设置"))
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(crate::settings::SettingsRequest::Open(Some(
                                        Category::Ai,
                                    )))
                                })),
                        ),
                )
                .child(self.composer(cx));
        }
        let elapsed = self.started.map(|t| t.elapsed()).unwrap_or(self.elapsed);
        let running = if self.stopping {
            "正在停止"
        } else if self.messages.last().is_some_and(|message| message.user) {
            "正在准备"
        } else {
            "正在回答"
        };
        let status = if self.busy {
            format!("{running} · {}", duration_label(elapsed))
        } else if elapsed > Duration::ZERO {
            format!("{} · 用时 {}", self.status, duration_label(elapsed))
        } else {
            self.status.clone()
        };
        shell
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("new-thread")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip(crate::locale::t(cx, "新会话"))
                            .tooltip(crate::locale::t(
                                cx,
                                if self.busy {
                                    "请先停止当前回答"
                                } else {
                                    "开始新会话"
                                },
                            ))
                            .disabled(self.busy || self.connecting)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(connection) = &this.connection
                                    && let Err(error) = connection.send(ClientCommand::NewThread)
                                {
                                    this.status = error;
                                    cx.notify();
                                    return;
                                }
                                this.messages.clear();
                                this.history_notice = None;
                                this.thread_id = None;
                                this.history_interrupted = false;
                                this.restored_model = None;
                                this.restored_effort = None;
                                this.finish_timing();
                                this.elapsed = Duration::ZERO;
                                this.resume_follow();
                                this.status = "新会话".into();
                                this.persist_history(true, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .child(
                                Button::new("expand-ai")
                                    .ghost()
                                    .icon(if self.zoomed {
                                        IconName::Minimize
                                    } else {
                                        IconName::Maximize
                                    })
                                    .tooltip(crate::locale::t(
                                        cx,
                                        if self.zoomed {
                                            "恢复 AI 面板"
                                        } else {
                                            "扩大 AI 面板"
                                        },
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let _ = this;
                                        cx.emit(crate::settings::SettingsRequest::ToggleAiZoom);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("chat-settings")
                                    .ghost()
                                    .icon(IconName::Settings)
                                    .tooltip(crate::locale::t(cx, "AI 设置"))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(crate::settings::SettingsRequest::Open(Some(
                                            Category::Ai,
                                        )))
                                    })),
                            ),
                    ),
            )
            .when_some(self.history_notice.clone(), |el, notice| {
                el.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_size(rems(crate::typography::META))
                        .text_color(cx.theme().muted_foreground)
                        .child(notice),
                )
            })
            .when(!self.ready, |el| {
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .p_3()
                        .bg(cx.theme().muted)
                        .child(IconName::Info)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(rems(crate::typography::META))
                                .child(crate::locale::t(cx, &self.status)),
                        )
                        .child(
                            Button::new("retry-connect")
                                .ghost()
                                .icon(IconName::Redo)
                                .w(px(32.))
                                .h(px(32.))
                                .p_0()
                                .tooltip(crate::locale::t(cx, "连接 / 重试"))
                                .disabled(self.connecting)
                                .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                        ),
                )
            })
            .child(
                div()
                    .id("ai-conversation")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .v_flex()
                    .gap_5()
                    .p_4()
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        let down = event.delta.pixel_delta(px(16.)).y < px(0.);
                        this.follow = false;
                        cx.notify();
                        let view = cx.entity().downgrade();
                        window.on_next_frame(move |_, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if down
                                    && !this.frozen
                                    && (this.scroll.max_offset().height + this.scroll.offset().y)
                                        <= px(24.)
                                {
                                    this.follow = true;
                                }
                                cx.notify();
                            });
                        });
                    }))
                    .capture_any_mouse_down(cx.listener(
                        |this, event: &MouseDownEvent, window, cx| {
                            if event.button == MouseButton::Right {
                                this.frozen = true;
                                this.follow = false;
                                this.selection_menu = Some((event.position, window.focused(cx)));
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.notify();
                                return;
                            }
                            if event.button == MouseButton::Left {
                                this.selection_menu = None;
                                this.frozen = true;
                                this.follow = false;
                                cx.notify();
                            }
                        },
                    ))
                    .when(self.messages.is_empty(), |el| {
                        el.child(
                            div()
                                .v_flex()
                                .gap_2()
                                .py_6()
                                .child(
                                    div().h_flex().gap_2().child(IconName::Bot).child(
                                        div()
                                            .text_size(rems(crate::typography::HEADING))
                                            .child(crate::locale::t(cx, "一起理清你的数据问题")),
                                    ),
                                )
                                .child(div().text_color(cx.theme().muted_foreground).child(
                                    crate::locale::t(
                                        cx,
                                        "从一段 SQL、一个错误，或一个分析目标开始。",
                                    ),
                                )),
                        )
                    })
                    .children(self.messages.iter().enumerate().map(|(index, message)| {
                        let text = message.visible.clone();
                        div()
                            .v_flex()
                            .gap_2()
                            .min_w_0()
                            .when(message.user, |el| {
                                el.p_3().rounded_lg().bg(cx.theme().muted)
                            })
                            .child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .text_size(rems(crate::typography::META))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if message.user {
                                        IconName::CircleUser
                                    } else {
                                        IconName::Bot
                                    })
                                    .child(if message.user {
                                        "你"
                                    } else {
                                        self.engine.name()
                                    }),
                            )
                            .child(
                                div()
                                    .id(("message-body", index))
                                    .w_full()
                                    .overflow_x_scroll()
                                    .child(
                                        TextView::markdown(
                                            ("ai-message", index),
                                            message.visible.clone(),
                                            window,
                                            cx,
                                        )
                                        .selectable(true)
                                        .code_block_actions(move |block, _, cx| {
                                            let code = block.code().to_string();
                                            let pointer_code = code.clone();
                                            div().id(("code-actions", index)).occlude().child(
                                                Button::new(("copy-code", index))
                                                    .ghost()
                                                    .icon(IconName::Copy)
                                                    .tooltip(crate::locale::t(cx, "复制代码"))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, window, cx| {
                                                            window.prevent_default();
                                                            cx.stop_propagation();
                                                            cx.write_to_clipboard(
                                                                ClipboardItem::new_string(
                                                                    pointer_code.clone(),
                                                                ),
                                                            );
                                                        },
                                                    )
                                                    .on_click(move |_, _, cx| {
                                                        cx.stop_propagation();
                                                        cx.write_to_clipboard(
                                                            ClipboardItem::new_string(code.clone()),
                                                        )
                                                    }),
                                            )
                                        }),
                                    ),
                            )
                            .child(
                                div().h_flex().child(
                                    Button::new(("copy", index))
                                        .ghost()
                                        .icon(IconName::Copy)
                                        .tooltip(crate::locale::t(cx, "复制全文"))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                text.clone(),
                                            ))
                                        }),
                                ),
                            )
                    })),
            )
            .when_some(self.selection_menu.clone(), |el, (position, focus)| {
                el.child(
                    deferred(
                        anchored()
                            .position(position)
                            .snap_to_window_with_margin(px(8.))
                            .child(
                                div()
                                    .id("selection-copy-menu")
                                    .occlude()
                                    .p_1()
                                    .w(px(160.))
                                    .bg(cx.theme().background)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .rounded_md()
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.selection_menu = None;
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .id("copy-selected")
                                            .p_2()
                                            .cursor_pointer()
                                            .child(crate::locale::t(cx, "复制选中内容"))
                                            .capture_any_mouse_down(cx.listener(
                                                move |this, event: &MouseDownEvent, window, cx| {
                                                    window.prevent_default();
                                                    cx.stop_propagation();
                                                    if event.button == MouseButton::Left {
                                                        if let Some(focus) = &focus {
                                                            window.focus(focus);
                                                        }
                                                        window.dispatch_action(
                                                            Box::new(gpui_component::input::Copy),
                                                            cx,
                                                        );
                                                        this.selection_menu = None;
                                                        cx.notify();
                                                    }
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .with_priority(2),
                )
            })
            .when(show_latest(self.messages.len(), self.follow), |el| {
                el.child(
                    div().h_flex().justify_end().px_3().child(
                        Button::new("latest")
                            .icon(IconName::ArrowDown)
                            .label(crate::locale::t(
                                cx,
                                if self.frozen {
                                    "最新 · 恢复显示"
                                } else {
                                    "最新"
                                },
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.resume_follow();
                                cx.notify();
                            })),
                    ),
                )
            })
            .when(
                show_activity(self.busy, self.ready, &self.status, &self.account_status),
                |el| {
                    el.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_size(rems(crate::typography::META))
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::locale::t(cx, &status)),
                    )
                },
            )
            .child(self.composer(cx))
    }
}

impl Drop for ChatView {
    fn drop(&mut self) {
        if !self.messages.is_empty() {
            self.history.save(self.history_snapshot());
        }
    }
}

fn show_latest(message_count: usize, following: bool) -> bool {
    message_count > 0 && !following
}
fn show_activity(busy: bool, ready: bool, status: &str, account_status: &str) -> bool {
    busy || (ready && !status.is_empty() && status != account_status && status != "新会话")
}
#[cfg(test)]
mod presentation_tests {
    use super::{show_activity, show_latest};
    #[test]
    fn empty_chat_never_offers_latest_even_when_following_is_paused() {
        assert!(!show_latest(0, false));
        assert!(!show_latest(1, true));
        assert!(show_latest(1, false));
    }
    #[test]
    fn readiness_is_quiet_but_generation_and_errors_remain_visible() {
        assert!(!show_activity(false, true, "账号已就绪", "账号已就绪"));
        assert!(!show_activity(
            false,
            true,
            "Account ready",
            "Account ready"
        ));
        assert!(show_activity(true, true, "Account ready", "Account ready"));
        assert!(show_activity(
            false,
            true,
            "Request failed",
            "Account ready"
        ));
        assert!(!show_activity(false, true, "新会话", "账号已就绪"));
    }
}
