use crate::panels::WorkbenchPanel;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable, StyledExt, Theme, ThemeMode, TitleBar, WindowExt,
    button::{Button, ButtonVariants},
    dock::{DockArea, DockEvent, DockItem, DockPlacement},
};
use std::sync::Arc;

const REGIONS: [DockPlacement; 3] = [
    DockPlacement::Left,
    DockPlacement::Bottom,
    DockPlacement::Right,
];

pub struct Workspace {
    dock: Entity<DockArea>,
    navigation: Entity<WorkbenchPanel>,
    connections: bool,
    focus_snapshot: Option<([bool; 3], Option<FocusHandle>)>,
    _dock_subscription: Subscription,
}

fn icon_button(id: &'static str, icon: impl Into<Icon>, tooltip: &'static str) -> Button {
    Button::new(id)
        .ghost()
        .icon(icon)
        .tooltip(tooltip)
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
                "项目",
                "入门工作台.note",
                "仅管理 .note 文件。当前为示例文档，文件管理将在后续接入。",
                cx,
            )
        });
        let ai = cx.new(|cx| {
            WorkbenchPanel::info(
                "ai-chat",
                "AI Chat",
                "让 AI 协助分析",
                "AI 服务尚未配置。接入后，可引用选中的 SQL、结构和结果摘要。",
                cx,
            )
        });
        let properties = cx.new(|cx| {
            WorkbenchPanel::info(
                "properties",
                "属性",
                "入门工作台.note",
                "包含 SQL、Markdown 与任务。当前为内存示例，尚未保存到磁盘。",
                cx,
            )
        });
        let schema = cx.new(|cx| {
            WorkbenchPanel::info(
                "schema",
                "数据库结构",
                "先选择数据库连接",
                "这里将展示表、视图、函数、字段、索引及 DDL。",
                cx,
            )
        });
        let output = cx.new(|cx| {
            WorkbenchPanel::info(
                "ai-output",
                "AI 运行输出",
                "暂无运行记录",
                "AI 请求状态、执行日志与错误信息将在这里展示。",
                cx,
            )
        });
        let center = DockItem::tab(notebook, &weak, window, cx);
        let left = DockItem::tab(navigation.clone(), &weak, window, cx);
        let right = DockItem::tabs(
            vec![Arc::new(ai), Arc::new(properties), Arc::new(schema)],
            &weak,
            window,
            cx,
        );
        let bottom = DockItem::tab(output, &weak, window, cx);
        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_left_dock(left, Some(px(220.)), true, window, cx);
            dock.set_right_dock(right, Some(px(300.)), true, window, cx);
            dock.set_bottom_dock(bottom, Some(px(160.)), false, window, cx);
            dock.set_locked(true, window, cx);
            dock.set_toggle_button_visible(false, cx);
        });
        let subscription = cx.subscribe(&dock, |_, _, event, cx| {
            if matches!(event, DockEvent::LayoutChanged) {
                cx.notify();
            }
        });
        Self {
            dock,
            navigation,
            connections: false,
            focus_snapshot: None,
            _dock_subscription: subscription,
        }
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

    fn settings(window: &mut Window, cx: &mut App) {
        window.open_dialog(cx, |dialog, _, cx| {
            let dark = cx.theme().mode.is_dark();
            dialog.title("设置").child(
                div()
                    .v_flex()
                    .gap_4()
                    .child("外观主题")
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("light")
                                    .icon(IconName::Sun)
                                    .label("白天 Light")
                                    .selected(!dark)
                                    .on_click(|_, window, cx| {
                                        Theme::change(ThemeMode::Light, Some(window), cx)
                                    }),
                            )
                            .child(
                                Button::new("dark")
                                    .icon(IconName::Moon)
                                    .label("黑夜 Dark")
                                    .selected(dark)
                                    .on_click(|_, window, cx| {
                                        Theme::change(ThemeMode::Dark, Some(window), cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("主题选择在当前会话中生效。"),
                    ),
            )
        });
    }

    fn help(window: &mut Window, cx: &mut App) {
        window.open_dialog(cx, |dialog, _, _| {
            dialog.title("使用帮助").child(
                div()
                    .v_flex()
                    .gap_3()
                    .child("项目 / 连接：点击切换侧栏，再次点击当前项收起。")
                    .child("顶部布局图标：显示或隐藏左侧栏、运行输出与右侧辅助面板。")
                    .child("专注模式：收起所有辅助区域，再次点击恢复原布局。")
                    .child("太阳 / 月亮：切换 Light / Dark 主题。")
                    .child("当前编辑仅保留在内存中，关闭窗口后丢失。"),
            )
        });
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                icon_button("project", IconName::Folder, "项目")
                    .selected(left && !self.connections)
                    .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx))),
            )
            .child(
                icon_button(
                    "connections",
                    Icon::default().path("icons/database.svg"),
                    "连接",
                )
                .selected(left && self.connections)
                .on_click(cx.listener(|this, _, window, cx| this.navigate(true, window, cx))),
            )
            .child(div().flex_1())
            .child(
                icon_button("settings", IconName::Settings, "设置")
                    .on_click(|_, window, cx| Self::settings(window, cx)),
            )
            .child(
                icon_button("help", IconName::Info, "帮助")
                    .on_click(|_, window, cx| Self::help(window, cx)),
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
            .child(div().flex_1().pl_2().text_sm().child("TurboDbNote"))
            .child(
                icon_button("left", IconName::PanelLeft, "显示 / 隐藏左侧栏")
                    .selected(left)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_region(DockPlacement::Left, window, cx)
                    })),
            )
            .child(
                icon_button("bottom", IconName::PanelBottom, "显示 / 隐藏运行输出")
                    .selected(bottom)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_region(DockPlacement::Bottom, window, cx)
                    })),
            )
            .child(
                icon_button("right", IconName::PanelRight, "显示 / 隐藏辅助面板")
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
                        "退出专注模式"
                    } else {
                        "进入专注模式"
                    },
                )
                .selected(focused)
                .on_click(cx.listener(|this, _, window, cx| this.toggle_focus(window, cx))),
            )
            .child(
                icon_button(
                    "theme",
                    if dark { IconName::Sun } else { IconName::Moon },
                    if dark {
                        "切换白天 Light"
                    } else {
                        "切换黑夜 Dark"
                    },
                )
                .on_click(|_, window, cx| {
                    let mode = if cx.theme().mode.is_dark() {
                        ThemeMode::Light
                    } else {
                        ThemeMode::Dark
                    };
                    Theme::change(mode, Some(window), cx);
                }),
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
                                    .px_3()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("未连接数据库    ·    内存示例    ·    SQL Notebook"),
                            )
                        }),
                ),
            )
    }
}
