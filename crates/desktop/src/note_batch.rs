use super::*;
use std::collections::VecDeque;

pub(super) struct Batch {
    token: Uuid,
    queue: VecDeque<(EntityId, crate::cell::BatchScope)>,
    active: Option<EntityId>,
    completed: usize,
    total: usize,
}
impl NoteView {
    pub(super) fn batch_label(&self, cx: &App) -> SharedString {
        self.batch
            .as_ref()
            .map(|b| {
                crate::locale::tr(
                    cx,
                    "notes.batch_progress",
                    &[
                        ("done", &b.completed.to_string()),
                        ("total", &b.total.to_string()),
                    ],
                )
            })
            .unwrap_or_else(|| {
                if self.batch_notice.is_empty() {
                    t(cx, "notes.run_all")
                } else {
                    self.batch_notice.clone().into()
                }
            })
    }
    pub(super) fn active_queries(&self, cx: &App) -> bool {
        self.batch.is_some() || self.cells.iter().any(|cell| cell.read(cx).working(cx))
    }
    pub(super) fn run_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_queries(cx) {
            return;
        }
        let mut queue = VecDeque::new();
        for cell in &self.cells {
            match cell.read(cx).batch_scope(cx) {
                Ok(Some(scope)) => queue.push_back((cell.entity_id(), scope)),
                Ok(None) => {}
                Err(error) => {
                    feedback::error(error, cx);
                    return;
                }
            }
        }
        if queue.is_empty() {
            feedback::error(t(cx, "notes.batch_empty"), cx);
            return;
        }
        self.batch_notice.clear();
        self.batch = Some(Batch {
            token: Uuid::new_v4(),
            total: queue.len(),
            queue,
            active: None,
            completed: 0,
        });
        self.next_batch(window, cx);
    }
    fn next_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = self.batch.as_mut() else {
            return;
        };
        let Some((id, scope)) = batch.queue.pop_front() else {
            self.batch_notice = crate::locale::tr(
                cx,
                "notes.batch_done",
                &[("count", &batch.completed.to_string())],
            )
            .to_string();
            self.batch = None;
            feedback::success(self.batch_notice.clone(), cx);
            cx.notify();
            return;
        };
        batch.active = Some(id);
        let token = batch.token;
        let cell = self.cells.iter().find(|c| c.entity_id() == id).cloned();
        if cell.is_none_or(|cell| {
            !cell.update(cx, |cell, cx| cell.start_batch(token, scope, window, cx))
        }) {
            self.batch = None;
            self.batch_notice = t(cx, "notes.batch_changed").to_string();
            feedback::error(self.batch_notice.clone(), cx);
        }
        cx.notify();
    }
    pub(super) fn batch_finished(
        &mut self,
        id: EntityId,
        token: Uuid,
        success: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(batch) = self.batch.as_mut() else {
            return;
        };
        if batch.token != token || batch.active != Some(id) {
            return;
        }
        if !success {
            self.batch_notice = t(cx, "notes.batch_stopped").to_string();
            self.batch = None;
        } else {
            batch.completed += 1;
            batch.active = None;
            cx.defer_in(window, |this, window, cx| this.next_batch(window, cx));
        }
        cx.notify();
    }
    pub(super) fn stop_queries(&mut self, cx: &mut Context<Self>) {
        self.batch = None;
        self.batch_notice = t(cx, "notes.batch_stopped").to_string();
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.stop(cx));
        }
        cx.notify();
    }
    pub(super) fn collapse_all(
        &mut self,
        collapsed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for cell in &self.cells {
            cell.update(cx, |cell, cx| cell.set_collapsed(collapsed, cx));
        }
        self.focus.focus(window);
        cx.notify();
    }
}
