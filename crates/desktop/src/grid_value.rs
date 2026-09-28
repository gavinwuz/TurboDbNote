use super::*;
use chrono::{DateTime, FixedOffset, Local, NaiveDate, Timelike};
use gpui_component::{
    calendar::{Calendar, CalendarEvent, CalendarState, Date},
    popover::Popover,
};
use std::cell::Cell;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Temporal {
    Text,
    Date,
    Time,
    DateTime,
}
pub(super) fn temporal(ty: &str) -> Temporal {
    let ty = ty.to_lowercase();
    if ty.contains("timestamp") || ty.contains("datetime") {
        Temporal::DateTime
    } else if ty.starts_with("date") {
        Temporal::Date
    } else if ty.starts_with("time") {
        Temporal::Time
    } else {
        Temporal::Text
    }
}
fn split_datetime(value: &str) -> (&str, &str, char) {
    value
        .find(['T', ' '])
        .map(|i| (&value[..i], &value[i + 1..], value.as_bytes()[i] as char))
        .unwrap_or((value, "", ' '))
}
fn parsed_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(split_datetime(value).0, "%Y-%m-%d").ok()
}
fn with_date(original: &str, date: NaiveDate, kind: Temporal) -> String {
    let date = date.format("%Y-%m-%d").to_string();
    if kind == Temporal::Date {
        return date;
    }
    let (_, time, separator) = split_datetime(original);
    format!(
        "{date}{separator}{}",
        if time.is_empty() { "00:00:00" } else { time }
    )
}
fn time_zone(time: &str) -> (&str, Option<FixedOffset>) {
    let zone = time
        .char_indices()
        .find(|(i, c)| *i >= 8 && matches!(c, '+' | '-' | 'Z' | 'z'))
        .map(|(i, _)| &time[i..])
        .unwrap_or("");
    if zone.eq_ignore_ascii_case("z") {
        return (zone, FixedOffset::east_opt(0));
    }
    let digits = zone.get(1..).unwrap_or("").replace(':', "");
    let offset = if digits.len() == 2 || digits.len() == 4 || digits.len() == 6 {
        let hour = digits.get(..2).and_then(|s| s.parse::<i32>().ok());
        let minute = if digits.len() >= 4 {
            digits.get(2..4).and_then(|s| s.parse::<i32>().ok())
        } else {
            Some(0)
        };
        let second = if digits.len() == 6 {
            digits.get(4..6).and_then(|s| s.parse::<i32>().ok())
        } else {
            Some(0)
        };
        match (hour, minute, second) {
            (Some(h), Some(m), Some(s)) if m < 60 && s < 60 => FixedOffset::east_opt(
                (h * 3600 + m * 60 + s) * if zone.starts_with('-') { -1 } else { 1 },
            ),
            _ => None,
        }
    } else {
        None
    };
    (zone, offset)
}
fn current_value(
    kind: Temporal,
    source: &str,
    precision: usize,
    with_zone: bool,
    now: DateTime<FixedOffset>,
) -> String {
    if kind == Temporal::Date {
        return now.format("%Y-%m-%d").to_string();
    }
    let (_, time, separator) = if kind == Temporal::DateTime {
        split_datetime(source)
    } else {
        ("", source, ' ')
    };
    let (zone, offset) = time_zone(time);
    let now = offset
        .map(|offset| now.with_timezone(&offset))
        .unwrap_or(now);
    let digits = time
        .find('.')
        .map(|i| {
            time[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .count()
        })
        .unwrap_or(precision)
        .min(9);
    let fraction = if digits == 0 {
        String::new()
    } else {
        format!(".{:09}", now.nanosecond())[..digits + 1].to_string()
    };
    let zone = if !zone.is_empty() {
        zone.to_string()
    } else if with_zone {
        now.format("%:z").to_string()
    } else {
        String::new()
    };
    let time = format!("{}{fraction}{zone}", now.format("%H:%M:%S"));
    if kind == Temporal::Time {
        time
    } else {
        format!("{}{separator}{time}", now.format("%Y-%m-%d"))
    }
}
pub(super) struct ValueInput {
    pub input: Entity<InputState>,
    date: Option<Entity<InputState>>,
    time: Option<Entity<InputState>>,
    calendar: Option<Entity<CalendarState>>,
    open: Rc<Cell<bool>>,
    kind: Temporal,
    multiline: bool,
    precision: usize,
    with_zone: bool,
    _subscriptions: Vec<Subscription>,
}
impl ValueInput {
    pub fn new(
        data_type: &str,
        value: String,
        multiline: bool,
        filter: bool,
        window: &mut Window,
        cx: &mut Context<TableView>,
    ) -> Self {
        let kind = temporal(data_type);
        let multiline = multiline && kind == Temporal::Text;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(multiline)
                .default_value(value.clone())
                .placeholder(match kind {
                    Temporal::Date => "YYYY-MM-DD".into(),
                    Temporal::Time => "HH:mm:ss".into(),
                    _ => t(
                        cx,
                        if filter {
                            "grid.filter.value"
                        } else {
                            "grid.value"
                        },
                    ),
                })
        });
        let (day, clock, _) = split_datetime(&value);
        let date = match kind {
            Temporal::Date => Some(input.clone()),
            Temporal::DateTime => Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(day.to_string())
                    .placeholder("YYYY-MM-DD")
            })),
            _ => None,
        };
        let time = match kind {
            Temporal::Time => Some(input.clone()),
            Temporal::DateTime => Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(clock.to_string())
                    .placeholder("HH:mm:ss")
            })),
            _ => None,
        };
        let calendar = date.as_ref().map(|_| {
            cx.new(|cx| {
                let mut picker = CalendarState::new(window, cx);
                picker.set_date(Date::Single(parsed_date(&value)), window, cx);
                picker
            })
        });
        let open = Rc::new(Cell::new(false));
        let date_field = date.clone();
        let time_field = time.clone();
        let sync_calendar = calendar.clone();
        let mut subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            move |_, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value();
                    let (day, clock, _) = split_datetime(&value);
                    if kind == Temporal::DateTime {
                        for (field, text) in
                            [(date_field.as_ref(), day), (time_field.as_ref(), clock)]
                        {
                            if let Some(field) = field
                                && field.read(cx).value().as_str() != text
                            {
                                field.update(cx, |field, cx| {
                                    field.set_value(text.to_string(), window, cx)
                                });
                            }
                        }
                    }
                    if let Some(calendar) = &sync_calendar
                        && calendar.read(cx).date() != Date::Single(parsed_date(&value))
                    {
                        calendar.update(cx, |picker, cx| {
                            picker.set_date(Date::Single(parsed_date(&value)), window, cx)
                        });
                    }
                    cx.notify();
                } else if filter && matches!(event, InputEvent::PressEnter { .. }) {
                    cx.defer_in(window, |this, _, cx| this.apply_filter(cx));
                }
            },
        )];
        if kind == Temporal::DateTime {
            for field in [date.as_ref().unwrap(), time.as_ref().unwrap()] {
                let input = input.clone();
                let date = date.clone().unwrap();
                let time = time.clone().unwrap();
                subscriptions.push(cx.subscribe_in(
                    field,
                    window,
                    move |_, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::Change) {
                            let old = input.read(cx).value();
                            let separator = split_datetime(&old).2;
                            let date = date.read(cx).value();
                            let time = time.read(cx).value();
                            let value = if time.is_empty() {
                                date.to_string()
                            } else {
                                format!("{date}{separator}{time}")
                            };
                            if old.as_str() != value {
                                input.update(cx, |field, cx| field.set_value(value, window, cx));
                            }
                        } else if filter && matches!(event, InputEvent::PressEnter { .. }) {
                            cx.defer_in(window, |this, _, cx| this.apply_filter(cx));
                        }
                    },
                ));
            }
        }
        if let Some(calendar) = &calendar {
            let input = input.clone();
            let open = open.clone();
            subscriptions.push(cx.subscribe_in(
                calendar,
                window,
                move |_, _, event: &CalendarEvent, window, cx| {
                    let CalendarEvent::Selected(Date::Single(Some(date))) = event else {
                        return;
                    };
                    let value = with_date(input.read(cx).value().as_str(), *date, kind);
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                    open.set(false);
                    cx.notify();
                },
            ));
        }
        let ty = data_type.to_lowercase();
        let precision = ty
            .split_once('(')
            .and_then(|(_, s)| s.split_once(')'))
            .and_then(|(n, _)| n.parse::<usize>().ok())
            .unwrap_or(0)
            .min(9);
        let with_zone =
            ty.contains("with time zone") || ty.contains("timestamptz") || ty.contains("timetz");
        Self {
            input,
            date,
            time,
            calendar,
            open,
            kind,
            multiline,
            precision,
            with_zone,
            _subscriptions: subscriptions,
        }
    }
    pub fn render(&self, disabled: bool, cx: &App) -> AnyElement {
        if self.kind == Temporal::Text {
            let input = if self.multiline {
                crate::feedback::editable_multiline(&self.input, disabled, cx)
            } else {
                Input::new(&self.input)
                    .cleanable(!disabled)
                    .disabled(disabled)
            };
            return input
                .small()
                .w_full()
                .when(self.multiline, |input| {
                    input.h(px(
                        if self.input.read(cx).value().len() > 256
                            || self.input.read(cx).value().contains('\n')
                        {
                            240.
                        } else {
                            120.
                        },
                    ))
                })
                .into_any_element();
        }
        let input = self.input.clone();
        let kind = self.kind;
        let precision = self.precision;
        let with_zone = self.with_zone;
        let mut row = div().h_flex().flex_wrap().gap_1().items_center().min_w_0();
        if let Some(date) = &self.date {
            row = row.child(
                Input::new(date)
                    .cleanable(!disabled)
                    .small()
                    .w(px(116.))
                    .disabled(disabled),
            );
        }
        if let Some(calendar) = &self.calendar {
            let calendar = calendar.clone();
            let open = self.open.clone();
            row = row.child(
                Popover::new(("value-calendar", self.input.entity_id()))
                    .open(self.open.get())
                    .on_open_change(move |value, _, cx| {
                        open.set(*value);
                        cx.refresh_windows();
                    })
                    .trigger(
                        Button::new("open-calendar")
                            .small()
                            .ghost()
                            .icon(IconName::Calendar)
                            .tooltip(t(cx, "grid.date.calendar"))
                            .disabled(disabled),
                    )
                    .content(move |_, _, _| Calendar::new(&calendar).number_of_months(1)),
            );
        }
        if let Some(time) = &self.time {
            row = row.child(
                div()
                    .id(("value-time", self.input.entity_id()))
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(t(cx, "grid.date.time_hint"))
                            .build(window, cx)
                    })
                    .child(
                        Input::new(time)
                            .cleanable(!disabled)
                            .small()
                            .w(px(190.))
                            .disabled(disabled),
                    ),
            );
        }
        row.child(
            Button::new(("value-now", self.input.entity_id()))
                .ghost()
                .small()
                .icon(gpui_component::Icon::default().path("icons/current-time.svg"))
                .label(t(cx, "grid.date.now"))
                .disabled(disabled)
                .on_click(move |_, window, cx| {
                    let value = current_value(
                        kind,
                        input.read(cx).value().as_str(),
                        precision,
                        with_zone,
                        Local::now().fixed_offset(),
                    );
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }),
        )
        .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{Temporal, current_value, parsed_date, split_datetime, temporal, with_date};
    use chrono::{DateTime, NaiveDate};
    #[test]
    fn date_changes_preserve_time_precision_and_timezone() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        for (value, expected) in [
            (
                "2026-09-21T23:59:59.123456+08:00",
                "2026-10-02T23:59:59.123456+08:00",
            ),
            ("0000-00-00 12:13:14.5", "2026-10-02 12:13:14.5"),
            ("", "2026-10-02 00:00:00"),
        ] {
            assert_eq!(with_date(value, date, Temporal::DateTime), expected);
        }
        assert!(parsed_date("0000-00-00").is_none());
        assert!(temporal("timestamp(6) with time zone") == Temporal::DateTime);
        assert_eq!(split_datetime("2026- 12:13:14"), ("2026-", "12:13:14", ' '));
    }
    #[test]
    fn current_time_respects_offset_and_precision() {
        let now = DateTime::parse_from_rfc3339("2026-09-22T03:04:05.123456789+00:00").unwrap();
        assert_eq!(
            current_value(
                Temporal::DateTime,
                "1969-12-31T23:00:00.000000+08:00",
                0,
                false,
                now
            ),
            "2026-09-22T11:04:05.123456+08:00"
        );
        assert_eq!(
            current_value(Temporal::Time, "01:02:03.00-05:00", 0, false, now),
            "22:04:05.12-05:00"
        );
        assert_eq!(
            current_value(Temporal::Date, "", 0, false, now),
            "2026-09-22"
        );
        assert_eq!(
            current_value(Temporal::DateTime, "", 6, true, now),
            "2026-09-22 03:04:05.123456+00:00"
        );
    }
}
