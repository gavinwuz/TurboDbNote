use crate::{
    feedback,
    locale::{t, tr},
};
use chrono::{Local, NaiveTime, TimeZone};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    calendar::{Calendar, CalendarState, Date},
    checkbox::Checkbox,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use std::{cell::Cell as Flag, rc::Rc, time::Duration};
use turbodbn_core::{TaskItem, TaskStatus};
use uuid::Uuid;

pub struct Changed;
pub struct TaskList {
    items: Vec<TaskItem>,
    description: String,
    undo_batch: Vec<TaskItem>,
    legacy: (TaskStatus, Option<i64>, Option<i64>),
    pub note_name: String,
    _timer: Task<()>,
}
impl EventEmitter<Changed> for TaskList {}
impl TaskList {
    pub fn new(
        items: Vec<TaskItem>,
        description: String,
        legacy: (TaskStatus, Option<i64>, Option<i64>),
        cx: &mut Context<Self>,
    ) -> Self {
        let timer = cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(10))
                    .await;
                if view.update(cx, |this, cx| this.remind(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            items,
            description,
            undo_batch: vec![],
            legacy,
            note_name: String::new(),
            _timer: timer,
        }
    }
    pub fn snapshot(&self) -> (Vec<TaskItem>, String, TaskStatus) {
        let status = if self.items.is_empty() {
            self.legacy.0
        } else if self.items.iter().all(|item| item.completed) {
            TaskStatus::Completed
        } else if self.items.iter().any(|item| item.completed) {
            TaskStatus::InProgress
        } else {
            TaskStatus::Pending
        };
        (self.items.clone(), self.description.clone(), status)
    }
    pub fn text(&self) -> String {
        if self.items.is_empty() {
            return self.description.clone();
        }
        let mut lines = self
            .items
            .iter()
            .map(|item| {
                format!(
                    "- [{}] {}{}{}",
                    if item.completed { "x" } else { " " },
                    item.title,
                    if item.assignee.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", item.assignee)
                    },
                    item.due_at
                        .and_then(|due| Local.timestamp_opt(due, 0).single())
                        .map(|due| format!(" · {}", due.format("%Y-%m-%d %H:%M")))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        if !self.description.is_empty() {
            lines.push(self.description.clone());
        }
        lines.join("\n")
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(Changed);
        cx.notify();
    }
    fn remind(&mut self, cx: &mut Context<Self>) {
        let now = Local::now().timestamp();
        let mut delivered = false;
        for item in self
            .items
            .iter_mut()
            .filter(|item| item.needs_reminder(now))
            .take(5)
        {
            item.reminded_at = item.reminder_at();
            delivered = true;
            feedback::task_reminder(
                item.id,
                item.reminded_at.unwrap_or(now),
                tr(
                    cx,
                    "tasks.reminder_message",
                    &[
                        ("note", &self.note_name),
                        ("task", &item.title),
                        ("assignee", &item.assignee),
                    ],
                ),
                cx,
            );
        }
        if delivered {
            self.changed(cx);
        }
    }
    fn convert(&mut self, cx: &mut Context<Self>) {
        if self
            .description
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            > 1000
            || self.description.lines().any(|line| line.len() > 8192)
        {
            feedback::error(t(cx, "tasks.limit"), cx);
            return;
        }
        self.items = self
            .description
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(1000)
            .map(|line| {
                let title = line
                    .strip_prefix("- [ ] ")
                    .or_else(|| line.strip_prefix("- [x] "))
                    .or_else(|| line.strip_prefix("- "))
                    .unwrap_or(line);
                let mut item = TaskItem::new(title);
                item.completed =
                    self.legacy.0 == TaskStatus::Completed || line.starts_with("- [x] ");
                item.due_at = self.legacy.1;
                item.reminder_minutes = self
                    .legacy
                    .1
                    .zip(self.legacy.2)
                    .and_then(|(due, reminder)| due.checked_sub(reminder))
                    .map(|seconds| seconds / 60)
                    .filter(|minutes| (0..=10080).contains(minutes));
                item
            })
            .collect();
        self.changed(cx);
    }
    fn edit_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(self.description.clone())
        });
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let field = input.clone();
            let owner = owner.clone();
            dialog
                .title(t(cx, "tasks.description"))
                .w(px(520.))
                .overlay_closable(false)
                .child(feedback::editable_multiline(&input, false, cx).h(px(220.)))
                .child(
                    feedback::actions().child(feedback::close_button(cx)).child(
                        Button::new("save-task-description")
                            .primary()
                            .label(t(cx, "dbwork.save"))
                            .on_click(move |_, window, cx| {
                                let value = field.read(cx).value().to_string();
                                let _ = owner.update(cx, |this, cx| {
                                    this.description = value.clone();
                                    this.changed(cx);
                                });
                                window.close_dialog(cx);
                            }),
                    ),
                )
        });
    }
    fn edit(&mut self, id: Option<Uuid>, window: &mut Window, cx: &mut Context<Self>) {
        if id.is_none() && self.items.len() >= 1000 {
            feedback::error(t(cx, "tasks.limit"), cx);
            return;
        }
        let item = id
            .and_then(|id| self.items.iter().find(|item| item.id == id).cloned())
            .unwrap_or_else(|| TaskItem::new(""));
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(id.is_none())
                .default_value(item.title.clone())
                .placeholder(t(cx, "tasks.name"))
        });
        let assignee = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(item.assignee.clone())
                .placeholder(t(cx, "tasks.assignee"))
        });
        let due = item
            .due_at
            .and_then(|due| Local.timestamp_opt(due, 0).single());
        let calendar = cx.new(|cx| {
            let mut calendar = CalendarState::new(window, cx);
            calendar.set_date(
                Date::Single(Some(
                    due.map(|due| due.date_naive())
                        .unwrap_or_else(|| Local::now().date_naive()),
                )),
                window,
                cx,
            );
            calendar
        });
        let clock = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(
                    due.map(|due| due.format("%H:%M:%S").to_string())
                        .unwrap_or_else(|| "18:00".into()),
                )
                .placeholder("HH:mm")
        });
        let has_due = Rc::new(Flag::new(item.due_at.is_some()));
        let reminder = Rc::new(Flag::new(item.reminder_minutes));
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let due_toggle = has_due.clone();
            let remind_toggle = reminder.clone();
            let reminder_menu = reminder.clone();
            let field = title.clone();
            let person = assignee.clone();
            let picker = calendar.clone();
            let time = clock.clone();
            let deadline = has_due.clone();
            let alarm = reminder.clone();
            let owner = owner.clone();
            let original = item.clone();
            dialog
                .title(t(cx, "tasks.edit"))
                .w(px(400.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(if id.is_none() {
                            feedback::editable_multiline(&title, false, cx)
                                .h(px(110.))
                                .into_any_element()
                        } else {
                            Input::new(&title).cleanable(true).into_any_element()
                        })
                        .when(id.is_none(), |form| {
                            form.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t(cx, "tasks.batch_help")),
                            )
                        })
                        .child(Input::new(&assignee).cleanable(true))
                        .child(
                            Checkbox::new("task-deadline-enabled")
                                .label(t(cx, "tasks.deadline"))
                                .checked(has_due.get())
                                .on_click(move |checked, _, cx| {
                                    due_toggle.set(*checked);
                                    cx.refresh_windows();
                                }),
                        )
                        .when(has_due.get(), |el| {
                            el.child(Calendar::new(&calendar).number_of_months(1))
                                .child(Input::new(&clock).cleanable(true))
                        })
                        .child(
                            Checkbox::new("task-reminder-enabled")
                                .label(t(cx, "tasks.reminder"))
                                .checked(has_due.get() && reminder.get().is_some())
                                .disabled(!has_due.get())
                                .on_click(move |checked, _, cx| {
                                    remind_toggle.set(checked.then_some(30));
                                    cx.refresh_windows();
                                }),
                        )
                        .when(has_due.get() && reminder.get().is_some(), |el| {
                            el.child(
                                Button::new("task-reminder-offset")
                                    .label(reminder_label(reminder.get().unwrap_or(30), cx))
                                    .dropdown_caret(true)
                                    .dropdown_menu(move |mut menu, _, cx| {
                                        for minutes in [0, 5, 15, 30, 60] {
                                            let value = reminder_menu.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(reminder_label(minutes, cx))
                                                    .on_click(move |_, _, cx| {
                                                        value.set(Some(minutes));
                                                        cx.refresh_windows();
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(cx, "tasks.reminder_scope")),
                        ),
                )
                .footer(move |_, _, _, cx| {
                    let field = field.clone();
                    let person = person.clone();
                    let picker = picker.clone();
                    let time = time.clone();
                    let deadline = deadline.clone();
                    let alarm = alarm.clone();
                    let owner = owner.clone();
                    let original = original.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("save-task-item")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "dbwork.save"))
                                    .on_click(move |_, window, cx| {
                                        let name = field.read(cx).value().trim().to_string();
                                        if name.is_empty()
                                            || (id.is_some() && name.len() > 8192)
                                            || person.read(cx).value().len() > 1024
                                        {
                                            feedback::error(t(cx, "tasks.invalid_name"), cx);
                                            return;
                                        }
                                        let due = if deadline.get() {
                                            let Date::Single(Some(day)) = picker.read(cx).date()
                                            else {
                                                feedback::error(
                                                    t(cx, "tasks.invalid_deadline"),
                                                    cx,
                                                );
                                                return;
                                            };
                                            let clock = time.read(cx).value();
                                            let parsed =
                                                NaiveTime::parse_from_str(clock.trim(), "%H:%M")
                                                    .or_else(|_| {
                                                        NaiveTime::parse_from_str(
                                                            clock.trim(),
                                                            "%H:%M:%S",
                                                        )
                                                    });
                                            let Some(timestamp) = parsed
                                                .ok()
                                                .and_then(|clock| {
                                                    Local
                                                        .from_local_datetime(&day.and_time(clock))
                                                        .single()
                                                })
                                                .map(|due| due.timestamp())
                                            else {
                                                feedback::error(
                                                    t(cx, "tasks.invalid_deadline"),
                                                    cx,
                                                );
                                                return;
                                            };
                                            Some(timestamp)
                                        } else {
                                            None
                                        };
                                        let mut updated = original.clone();
                                        updated.title = name;
                                        updated.assignee =
                                            person.read(cx).value().trim().to_string();
                                        updated.due_at = due;
                                        updated.reminder_minutes = due.and(alarm.get());
                                        if updated.reminder_at() != original.reminder_at() {
                                            updated.reminded_at = None;
                                        }
                                        if id.is_none() {
                                            let Some(mut batch) = parse_task_lines(&updated.title)
                                            else {
                                                feedback::error(t(cx, "tasks.limit"), cx);
                                                return;
                                            };
                                            for item in &mut batch {
                                                item.assignee = updated.assignee.clone();
                                                item.due_at = updated.due_at;
                                                item.reminder_minutes = updated.reminder_minutes;
                                            }
                                            let count = batch.len();
                                            let added = owner
                                                .update(cx, |this, cx| {
                                                    if this.items.len() + count > 1000 {
                                                        return false;
                                                    }
                                                    this.undo_batch = batch.clone();
                                                    this.items.extend(batch);
                                                    this.changed(cx);
                                                    true
                                                })
                                                .unwrap_or(false);
                                            if !added {
                                                feedback::error(t(cx, "tasks.limit"), cx);
                                                return;
                                            }
                                            feedback::success(
                                                tr(
                                                    cx,
                                                    "tasks.batch_added",
                                                    &[("count", &count.to_string())],
                                                ),
                                                cx,
                                            );
                                            window.close_dialog(cx);
                                            return;
                                        }
                                        let _ = owner.update(cx, |this, cx| {
                                            if let Some(existing) = this
                                                .items
                                                .iter_mut()
                                                .find(|item| item.id == updated.id)
                                            {
                                                if existing.reminder_at() == updated.reminder_at() {
                                                    updated.reminded_at = existing.reminded_at;
                                                }
                                                *existing = updated.clone();
                                            } else {
                                                this.items.push(updated.clone());
                                            }
                                            this.changed(cx);
                                        });
                                        feedback::success(t(cx, "tasks.updated"), cx);
                                        window.close_dialog(cx);
                                    }),
                            ),
                    ]
                })
        });
    }
}
fn reminder_label(minutes: i64, cx: &App) -> SharedString {
    if minutes == 0 {
        t(cx, "tasks.remind_at_deadline")
    } else {
        tr(
            cx,
            "tasks.remind_before",
            &[("minutes", &minutes.to_string())],
        )
    }
}
fn completed_text(text: String, completed: bool, cx: &App) -> StyledText {
    let length = text.len();
    StyledText::new(text).with_highlights([(
        0..length,
        HighlightStyle {
            color: completed.then_some(cx.theme().muted_foreground),
            strikethrough: completed.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..Default::default()
        },
    )])
}
impl Render for TaskList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().v_flex().gap_1().min_w_0();
        for item in self.items.clone() {
            let id = item.id;
            let owner = item.assignee.clone();
            let due = item
                .due_at
                .and_then(|due| Local.timestamp_opt(due, 0).single())
                .map(|due| due.format("%m-%d %H:%M").to_string());
            body = body.child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .child(
                        Checkbox::new(SharedString::from(format!("complete-task-{id}")))
                            .checked(item.completed)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(item) = this.items.iter_mut().find(|item| item.id == id)
                                {
                                    item.completed = *checked;
                                }
                                this.changed(cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("task-name-{id}")))
                            .ghost()
                            .small()
                            .child(completed_text(item.title.clone(), item.completed, cx))
                            .tooltip(t(cx, "tasks.edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit(Some(id), window, cx)
                            })),
                    )
                    .when(true, |row| {
                        row.child(
                            Button::new(SharedString::from(format!("task-assignee-{id}")))
                                .ghost()
                                .small()
                                .icon(IconName::User)
                                .child(completed_text(owner, item.completed, cx))
                                .tooltip(t(cx, "tasks.assignee"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit(Some(id), window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("task-due-{id}")))
                            .ghost()
                            .small()
                            .icon(IconName::Calendar)
                            .child(completed_text(
                                due.unwrap_or_else(|| t(cx, "tasks.set_deadline").to_string()),
                                item.completed,
                                cx,
                            ))
                            .tooltip(t(cx, "tasks.deadline"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit(Some(id), window, cx)
                            })),
                    )
                    .when(item.reminder_minutes.is_some(), |row| {
                        row.child(
                            Button::new(SharedString::from(format!("task-alarm-{id}")))
                                .ghost()
                                .small()
                                .icon(IconName::Bell)
                                .tooltip(reminder_label(item.reminder_minutes.unwrap_or(0), cx))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit(Some(id), window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("delete-task-{id}")))
                            .ghost()
                            .small()
                            .icon(IconName::Delete)
                            .tooltip(t(cx, "tasks.delete"))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                let owner = cx.entity().downgrade();
                                feedback::confirm(
                                    t(cx, "tasks.delete"),
                                    t(cx, "tasks.delete_body"),
                                    window,
                                    cx,
                                    move |_, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.items.retain(|item| item.id != id);
                                            this.changed(cx);
                                        });
                                    },
                                );
                            })),
                    ),
            );
        }
        if self.items.is_empty() && !self.description.is_empty() {
            body = body
                .child(div().whitespace_normal().child(self.description.clone()))
                .child(
                    Button::new("convert-legacy-task")
                        .ghost()
                        .icon(IconName::CircleCheck)
                        .label(t(cx, "tasks.convert"))
                        .on_click(cx.listener(|this, _, _, cx| this.convert(cx))),
                );
        }
        if !self.description.is_empty() {
            body = body.child(
                Button::new("edit-task-description")
                    .ghost()
                    .small()
                    .icon(IconName::File)
                    .label(t(cx, "tasks.description"))
                    .on_click(cx.listener(|this, _, window, cx| this.edit_description(window, cx))),
            );
        }
        body = body.when(!self.undo_batch.is_empty(), |body| {
            body.child(
                Button::new("undo-task-batch")
                    .ghost()
                    .small()
                    .icon(IconName::Undo)
                    .label(t(cx, "tasks.undo_batch"))
                    .disabled(
                        !self
                            .undo_batch
                            .iter()
                            .all(|original| self.items.iter().any(|item| item == original)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this
                            .undo_batch
                            .iter()
                            .all(|original| this.items.iter().any(|item| item == original))
                        {
                            let ids = std::mem::take(&mut this.undo_batch)
                                .into_iter()
                                .map(|item| item.id)
                                .collect::<Vec<_>>();
                            this.items.retain(|item| !ids.contains(&item.id));
                            this.changed(cx);
                        }
                    })),
            )
        });
        body.child(
            Button::new("add-task-item")
                .ghost()
                .small()
                .icon(IconName::Plus)
                .label(t(cx, "tasks.add"))
                .on_click(cx.listener(|this, _, window, cx| this.edit(None, window, cx))),
        )
    }
}

// Parse only new-task input: editing an existing title never splits it.
fn parse_task_lines(source: &str) -> Option<Vec<TaskItem>> {
    if source.len() > 8_192_000 {
        return None;
    }
    let mut items = vec![];
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if matches!(line, "-" | "*") {
            continue;
        }
        let body = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .unwrap_or(line);
        if matches!(body, "[ ]" | "[x]" | "[X]") {
            continue;
        }
        let (body, completed) = if let Some(body) = body
            .strip_prefix("[x] ")
            .or_else(|| body.strip_prefix("[X] "))
        {
            (body, true)
        } else {
            (body.strip_prefix("[ ] ").unwrap_or(body), false)
        };
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        if body.len() > 8192 || items.len() >= 1000 {
            return None;
        }
        let mut item = TaskItem::new(body);
        item.completed = completed;
        items.push(item);
    }
    (!items.is_empty()).then_some(items)
}
#[cfg(test)]
mod batch_tests {
    use super::parse_task_lines;
    #[test]
    fn batch_tasks_preserve_state_duplicates_and_inner_symbols() {
        let items = parse_task_lines("- aa\r\n* bb\n- [x] cc\n- [ ] aa\n\nplain - text\n").unwrap();
        assert_eq!(
            items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["aa", "bb", "cc", "aa", "plain - text"]
        );
        assert!(items[2].completed);
        assert!(!items[3].completed);
        assert_ne!(items[0].id, items[3].id);
        assert!(parse_task_lines(&"a\n".repeat(1001)).is_none());
        assert!(parse_task_lines(&"a".repeat(8193)).is_none());
        assert!(parse_task_lines("\n ").is_none());
        assert_eq!(
            parse_task_lines("- \n* \n- [ ]\n- [x]\naa").unwrap().len(),
            1
        );
    }
}
