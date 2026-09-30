use super::*;
use crate::{feedback::SqlScope, locale::t};
use turbodbn_services::database::notebook_query::COMMANDS;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum AccessMode {
    #[default]
    ReadOnly,
    ReadWrite,
    Admin,
}
impl AccessMode {
    fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "mcp.mode.read",
            Self::ReadWrite => "mcp.mode.write",
            Self::Admin => "mcp.mode.admin",
        }
    }
    pub fn allows(self, command: &str) -> bool {
        match command {
            "select" | "explain" | "show_create" | "show_columns" => true,
            "insert" | "update" | "delete" => self != Self::ReadOnly,
            _ => self == Self::Admin && COMMANDS.iter().any(|(key, _, _)| *key == command),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReviewState {
    Waiting,
    ConfirmWrite,
    Approved,
    Running,
    Done,
    Failed,
    Cancelled,
    Expired,
}
impl ReviewState {
    pub fn pending(self) -> bool {
        matches!(self, Self::Waiting | Self::ConfirmWrite | Self::Approved)
    }
    fn confirmed(self, rendered: Self, risky: bool) -> Self {
        if self != rendered {
            return self;
        }
        match self {
            Self::Waiting if risky => Self::ConfirmWrite,
            Self::Waiting | Self::ConfirmWrite => Self::Approved,
            _ => self,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Waiting => "mcp.card.waiting",
            Self::ConfirmWrite => "mcp.card.confirm_write",
            Self::Approved | Self::Running => "mcp.card.running",
            Self::Done => "mcp.card.done",
            Self::Failed => "mcp.card.failed",
            Self::Cancelled => "mcp.card.cancelled",
            Self::Expired => "mcp.card.expired",
        }
    }
}
pub(super) struct SqlReview {
    pub id: uuid::Uuid,
    pub anchor: usize,
    pub scope: SqlScope,
    pub connection_generation: u64,
    pub chat_generation: usize,
    pub turn: Option<Instant>,
    pub command: &'static str,
    pub sql: String,
    pub risky: bool,
    pub state: ReviewState,
    pub deadline: Instant,
    pub expanded: bool,
    pub result: Option<String>,
    pub elapsed_ms: u64,
    pub rows: usize,
}

impl ChatView {
    fn access_epoch(&self, cx: &App) -> Option<u64> {
        self.sql_scope
            .as_ref()
            .and_then(|s| s.connection)
            .and_then(|id| {
                self.databases
                    .as_ref()
                    .and_then(|db| db.upgrade())
                    .map(|db| db.read(cx).note_generation(id))
            })
    }
    pub(super) fn effective_sql_mode(&self, cx: &App) -> AccessMode {
        if self.access_scope == self.sql_scope
            && self.access_generation == self.generation
            && self.access_connection_generation == self.access_epoch(cx)
        {
            self.sql_access
        } else {
            AccessMode::ReadOnly
        }
    }
    pub(super) fn reset_sql_access(&mut self) {
        self.sql_access = AccessMode::ReadOnly;
        self.access_scope = None;
        self.access_connection_generation = None;
        for card in &mut self.sql_reviews {
            if card.state.pending() {
                card.state = ReviewState::Expired;
            } else if card.state == ReviewState::Running {
                card.state = ReviewState::Failed;
            }
        }
    }
    pub(super) fn review_current(&self, card: &SqlReview, cx: &App) -> bool {
        self.busy
            && !self.stopping
            && self.generation == card.chat_generation
            && self.started == card.turn
            && self.sql_scope.as_ref() == Some(&card.scope)
            && Instant::now() < card.deadline
            && card.scope.connection.is_some_and(|id| {
                self.databases
                    .as_ref()
                    .and_then(|db| db.upgrade())
                    .is_some_and(|db| {
                        db.read(cx).note_generation(id) == card.connection_generation
                            && db.read(cx).note_connection_state(id) == Some(true)
                    })
            })
    }
    pub(super) fn sql_access_control(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        let scope = self.sql_scope.clone();
        let generation = self.generation;
        let epoch = self.access_epoch(cx);
        let mode = self.effective_sql_mode(cx);
        Button::new("ai-sql-permissions")
            .ghost()
            .h(px(32.))
            .icon(Icon::default().path("icons/ai-permissions.svg").text_color(
                crate::assets::note_color(
                    match mode {
                        AccessMode::ReadOnly => 1,
                        AccessMode::ReadWrite => 0,
                        AccessMode::Admin => 3,
                    },
                    cx,
                ),
            ))
            .label(t(cx, mode.label()))
            .tooltip(t(cx, "mcp.mode.hint"))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, cx| {
                for mode in [
                    AccessMode::ReadOnly,
                    AccessMode::ReadWrite,
                    AccessMode::Admin,
                ] {
                    let owner = owner.clone();
                    let scope = scope.clone();
                    menu = menu.item(PopupMenuItem::new(t(cx, mode.label())).on_click(
                        move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.sql_scope != scope
                                    || this.generation != generation
                                    || this.access_epoch(cx) != epoch
                                {
                                    return;
                                }
                                this.sql_access = mode;
                                this.access_scope = scope.clone();
                                this.access_generation = generation;
                                this.access_connection_generation = this.access_epoch(cx);
                                // A changed permission never consumes an earlier confirmation.
                                for card in &mut this.sql_reviews {
                                    if card.state.pending() {
                                        card.state = ReviewState::Waiting;
                                    }
                                }
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            })
    }
    fn review_action(
        &mut self,
        id: uuid::Uuid,
        cancel: bool,
        expected: ReviewState,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.sql_reviews.iter().position(|c| c.id == id) else {
            return;
        };
        let card = &self.sql_reviews[index];
        if !card.state.pending() {
            return;
        }
        let current = self.review_current(card, cx);
        let permitted = self.effective_sql_mode(cx).allows(card.command)
            && cx
                .global::<Preferences>()
                .database
                .command_enabled(card.command);
        let card = &mut self.sql_reviews[index];
        card.state = if cancel {
            ReviewState::Cancelled
        } else if !current {
            ReviewState::Expired
        } else if !permitted {
            ReviewState::Waiting
        } else {
            card.state.confirmed(expected, card.risky)
        };
        if matches!(
            card.state,
            ReviewState::Approved | ReviewState::Cancelled | ReviewState::ConfirmWrite
        ) {
            self.resume_follow();
        }
        cx.notify();
    }
    pub(super) fn review_cards(
        &self,
        anchor: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.sql_reviews
            .iter()
            .filter(|card| card.anchor == anchor)
            .map(|card| {
                let id = card.id;
                let current = self.review_current(card, cx);
                let mode_ok = self.effective_sql_mode(cx).allows(card.command);
                let policy_ok = cx
                    .global::<Preferences>()
                    .database
                    .command_enabled(card.command);
                let pending = card.state.pending();
                let state = if pending && !current {
                    ReviewState::Expired
                } else {
                    card.state
                };
                let label = COMMANDS
                    .iter()
                    .find(|(key, _, _)| *key == card.command)
                    .map(|(_, label, _)| *label)
                    .unwrap_or("SQL");
                let mut body = div()
                    .id(SharedString::from(format!("sql-review-{id}")))
                    .w_full()
                    .min_w_0()
                    .v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(Icon::default().path("icons/ai-permissions.svg"))
                            .child(t(cx, state.label()))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_sm()
                            .whitespace_normal()
                            .child(format!("{} / {}", card.scope.name, card.scope.database)),
                    );
                if (pending && current) || card.expanded {
                    let fence = "~".repeat(
                        card.sql
                            .split(|c| c != '~')
                            .map(str::len)
                            .max()
                            .unwrap_or(0)
                            .max(2)
                            + 1,
                    );
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("sql-review-code-{id}")))
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .child(
                                TextView::markdown(
                                    SharedString::from(format!("sql-review-text-{id}")),
                                    format!("{fence}sql\n{}\n{fence}", card.sql),
                                    window,
                                    cx,
                                )
                                .w_full()
                                .selectable(true),
                            ),
                    );
                }
                if pending && current {
                    body = body.child(t(cx, "mcp.share_results"));
                    if !mode_ok {
                        body = body
                            .child(t(cx, "mcp.card.mode_blocked"))
                            .child(self.sql_access_control(cx));
                    }
                    if !policy_ok {
                        body = body.child(t(cx, "mcp.card.policy_blocked")).child(
                            Button::new("sql-policy-settings")
                                .ghost()
                                .icon(IconName::Settings)
                                .label(t(cx, "settings.sql_policy_open"))
                                .on_click(|_, _, cx| {
                                    let hub = cx.global::<crate::feedback::Hub>().0.clone();
                                    cx.defer(move |cx| {
                                        hub.update(cx, |_, cx| {
                                            cx.emit(crate::feedback::OpenSqlSettings)
                                        })
                                    });
                                }),
                        );
                    }
                    if state == ReviewState::ConfirmWrite {
                        body = body.child(
                            div()
                                .text_color(cx.theme().danger)
                                .whitespace_normal()
                                .child(t(cx, "mcp.card.risk")),
                        );
                    }
                    body = body.child(
                        crate::feedback::actions()
                            .child(
                                Button::new("sql-review-cancel")
                                    .icon(IconName::Close)
                                    .label(t(cx, "mcp.card.cancel"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.review_action(id, true, state, cx)
                                    })),
                            )
                            .child(
                                Button::new("sql-review-approve")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(
                                        cx,
                                        if state == ReviewState::ConfirmWrite {
                                            "mcp.card.confirm_execute"
                                        } else {
                                            "mcp.execute_share"
                                        },
                                    ))
                                    .disabled(
                                        !mode_ok || !policy_ok || state == ReviewState::Approved,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.review_action(id, false, state, cx)
                                    })),
                            ),
                    );
                } else {
                    if state == ReviewState::Done {
                        body = body.child(crate::locale::tr(
                            cx,
                            "mcp.card.summary",
                            &[
                                ("rows", &card.rows.to_string()),
                                ("ms", &card.elapsed_ms.to_string()),
                            ],
                        ));
                    }
                    body = body.child(
                        Button::new("sql-review-expand")
                            .ghost()
                            .icon(IconName::ChevronDown)
                            .label(t(cx, "mcp.card.details"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(card) = this.sql_reviews.iter_mut().find(|c| c.id == id)
                                {
                                    card.expanded = !card.expanded;
                                }
                                cx.notify();
                            })),
                    );
                    if card.expanded
                        && let Some(result) = &card.result
                    {
                        let markdown = format!("```json\n{result}\n```");
                        body = body.child(
                            div()
                                .id(SharedString::from(format!("sql-review-result-{id}")))
                                .max_h(px(280.))
                                .overflow_y_scroll()
                                .child(
                                    TextView::markdown(
                                        SharedString::from(format!("sql-review-result-text-{id}")),
                                        markdown,
                                        window,
                                        cx,
                                    )
                                    .w_full()
                                    .selectable(true),
                                ),
                        );
                    }
                }
                body.into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{AccessMode, ReviewState};
    #[test]
    fn writes_need_two_actions_and_terminal_requests_cannot_reactivate() {
        assert_eq!(
            ReviewState::Waiting.confirmed(ReviewState::Waiting, false),
            ReviewState::Approved
        );
        let first = ReviewState::Waiting.confirmed(ReviewState::Waiting, true);
        assert_eq!(first, ReviewState::ConfirmWrite);
        assert_eq!(
            first.confirmed(ReviewState::Waiting, true),
            ReviewState::ConfirmWrite
        );
        assert_eq!(
            first.confirmed(ReviewState::ConfirmWrite, true),
            ReviewState::Approved
        );
        for state in [
            ReviewState::Expired,
            ReviewState::Cancelled,
            ReviewState::Done,
            ReviewState::Running,
            ReviewState::Approved,
        ] {
            assert_eq!(state.confirmed(state, true), state);
        }
    }
    #[test]
    fn modes_do_not_expand_driver_or_command_capabilities() {
        for command in ["select", "explain", "show_create", "show_columns"] {
            assert!(AccessMode::ReadOnly.allows(command));
        }
        for command in ["insert", "update", "delete"] {
            assert!(!AccessMode::ReadOnly.allows(command));
            assert!(AccessMode::ReadWrite.allows(command));
        }
        for command in ["drop", "truncate", "create", "alter", "show_variables"] {
            assert!(!AccessMode::ReadWrite.allows(command));
            assert!(AccessMode::Admin.allows(command));
        }
        assert!(!AccessMode::Admin.allows("grant"));
        assert!(!AccessMode::Admin.allows("unknown"));
        assert!(!ReviewState::Running.pending());
        assert!(!ReviewState::Expired.pending());
    }
}
