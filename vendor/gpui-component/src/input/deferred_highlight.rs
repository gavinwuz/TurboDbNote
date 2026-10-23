//! Optional input-first highlighting. Text changes never wait for parsing or capture queries.
use super::{InputState, highlight_map::mapped_styles, mode::InputMode};
use crate::{
    ActiveTheme,
    highlighter::{HighlightTheme, SyntaxHighlighter},
};
use gpui::{App, Context, HighlightStyle, SharedString, Task};
use ropey::Rope;
use std::{
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
type Styles = Vec<(Range<usize>, HighlightStyle)>;
pub(super) struct DeferredHighlight {
    enabled: bool,
    revision: u64,
    due: Instant,
    composing: bool,
    task: Option<Task<()>>,
    worker: Arc<Mutex<Option<(SharedString, SyntaxHighlighter)>>>,
    light: SharedString,
    published: Option<(u64, Arc<HighlightTheme>, Styles)>,
    edits: Vec<(Range<usize>, usize)>,
}
impl DeferredHighlight {
    pub(super) fn theme(&self) -> Option<&Arc<HighlightTheme>> {
        self.published.as_ref().map(|(_, theme, _)| theme)
    }
    fn new(light: SharedString) -> Self {
        Self {
            enabled: true,
            revision: 0,
            due: Instant::now(),
            composing: false,
            task: None,
            worker: Arc::new(Mutex::new(None)),
            light,
            published: None,
            edits: vec![],
        }
    }
}
fn boundary(text: &str) -> bool {
    text.is_empty()
        || text.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '.' | ',' | ';' | '(' | ')' | '=' | '+' | '-' | '*' | '/' | '\'' | '"' | '`'
                )
        })
}
fn long_line(text: &Rope) -> bool {
    let mut width = 0;
    for byte in text.chunks().flat_map(str::bytes) {
        if matches!(byte, b'\n' | b'\r') {
            width = 0;
        } else {
            width += 1;
            if width > 8192 {
                return true;
            }
        }
    }
    false
}
impl InputState {
    /// Enable coalesced background highlighting; `light_language` is used for very long lines.
    pub fn deferred_highlighting(mut self, light_language: impl Into<SharedString>) -> Self {
        self.text_wrapper.incremental_wrap = true;
        self.deferred_highlight = Some(DeferredHighlight::new(light_language.into()));
        self
    }
    /// Pause visual analysis without changing text, history, cursor, or native layout optimizations.
    pub fn set_deferred_highlighting_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let Some(state) = self.deferred_highlight.as_mut() else {
            return;
        };
        if state.enabled == enabled {
            return;
        }
        state.enabled = enabled;
        state.revision = state.revision.wrapping_add(1);
        state.published = None;
        state.edits.clear();
        state.task = None;
        if enabled {
            self.schedule_highlight(" ", false, cx);
        }
        cx.notify();
    }
    pub(super) fn invalidate_highlight(&mut self) {
        if let Some(state) = &mut self.deferred_highlight {
            state.published = None;
            state.edits.clear();
        }
    }
    pub(super) fn edit_highlight(
        &mut self,
        range: Range<usize>,
        inserted: &str,
        composing: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = &mut self.deferred_highlight {
            if state.edits.len() >= 64 {
                state.published = None;
                state.edits.clear();
            }
            if state.published.is_some() {
                state.edits.push((range, inserted.len()));
            }
        }
        self.schedule_highlight(inserted, composing, cx);
    }
    pub(super) fn schedule_highlight(
        &mut self,
        inserted: &str,
        composing: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.deferred_highlight.as_mut() else {
            return;
        };
        if !state.enabled {
            return;
        }
        state.revision = state.revision.wrapping_add(1);
        state.composing = composing;
        state.due = Instant::now()
            + if boundary(inserted) && !composing {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(150)
            };
        if state.task.is_some() {
            return;
        }
        state.task = Some(cx.spawn(async move |input, cx| {
            loop {
                let delay = input
                    .read_with(cx, |input, _| {
                        input
                            .deferred_highlight
                            .as_ref()
                            .map(|state| state.due.saturating_duration_since(Instant::now()))
                    })
                    .ok()
                    .flatten();
                let Some(delay) = delay else {
                    return;
                };
                if !delay.is_zero() {
                    gpui::Timer::after(delay.min(Duration::from_millis(16))).await;
                    continue;
                }
                let snapshot = input
                    .update(cx, |input, cx| {
                        let state = input.deferred_highlight.as_mut()?;
                        if state.composing || input.ime_marked_range.is_some() {
                            state.task = None;
                            return None;
                        }
                        let InputMode::CodeEditor { language, .. } = &input.mode else {
                            state.task = None;
                            return None;
                        };
                        Some((
                            state.revision,
                            input.text.clone(),
                            language.clone(),
                            state.light.clone(),
                            cx.theme().highlight_theme.clone(),
                            state.worker.clone(),
                        ))
                    })
                    .ok()
                    .flatten();
                let Some((revision, text, language, light, theme, worker)) = snapshot else {
                    return;
                };
                let result_theme = theme.clone();
                let styles = cx
                    .background_executor()
                    .spawn(async move {
                        let language = if long_line(&text) { light } else { language };
                        let Ok(mut stored) = worker.lock() else {
                            return vec![];
                        };
                        if stored
                            .as_ref()
                            .is_none_or(|(current, _)| *current != language)
                        {
                            *stored = Some((language.clone(), SyntaxHighlighter::new(&language)));
                        }
                        let (_, highlighter) = stored.as_mut().unwrap();
                        highlighter.update_snapshot(&text);
                        // Capture work, including multiline context, runs off the UI thread too.
                        highlighter.styles(&(0..text.len()), &theme)
                    })
                    .await;
                let finished = input
                    .update(cx, |input, cx| {
                        let Some(state) = input.deferred_highlight.as_mut() else {
                            return true;
                        };
                        if state.revision != revision {
                            return false;
                        }
                        state.published = Some((revision, result_theme, styles));
                        state.edits.clear();
                        state.task = None;
                        cx.notify();
                        true
                    })
                    .unwrap_or(true);
                if finished {
                    return;
                }
            }
        }));
    }
    pub(super) fn deferred_styles(&self, range: Range<usize>, cx: &App) -> Option<Styles> {
        let state = self.deferred_highlight.as_ref()?;
        let (_, theme, styles) = state.published.as_ref()?;
        if theme != &cx.theme().highlight_theme {
            return None;
        }
        Some(mapped_styles(styles, &state.edits, range))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_include_sql_punctuation_but_not_identifier_characters() {
        assert!(boundary(" "));
        assert!(boundary("."));
        assert!(boundary("\n"));
        assert!(boundary("'"));
        assert!(!boundary("cq_itemtype"));
        assert!(!boundary("中"));
    }
    #[test]
    fn long_line_detection_is_worker_only_and_preserves_newline_boundaries() {
        assert!(long_line(&Rope::from("x".repeat(8193))));
        assert!(!long_line(&Rope::from(format!(
            "{}\n{}",
            "x".repeat(6000),
            "x".repeat(6000)
        ))));
    }
}
