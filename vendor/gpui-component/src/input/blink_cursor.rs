use std::time::{Duration, Instant};

use gpui::{Context, Pixels, Task, Timer, px};

static INTERVAL: Duration = Duration::from_millis(500);
static PAUSE_DELAY: Duration = Duration::from_millis(300);
pub(super) const CURSOR_WIDTH: Pixels = px(1.5);

/// To manage the Input cursor blinking.
///
/// It will start blinking with a interval of 500ms.
/// Every loop will notify the view to update the `visible`, and Input will observe this update to touch repaint.
///
/// The input painter will check if this in visible state, then it will draw the cursor.
pub(crate) struct BlinkCursor {
    visible: bool,
    paused: bool,
    last_pause: Instant,
    epoch: usize,

    _task: Task<()>,
}

impl BlinkCursor {
    pub fn new() -> Self {
        Self {
            visible: false,
            paused: false,
            last_pause: Instant::now(),
            epoch: 0,
            _task: Task::ready(()),
        }
    }

    /// Start the blinking
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.blink(self.epoch, cx);
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.next_epoch();
        self.paused = false;
        self.visible = false;
        self._task = Task::ready(());
        cx.notify();
    }

    fn next_epoch(&mut self) -> usize {
        self.epoch += 1;
        self.epoch
    }

    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if self.paused || epoch != self.epoch {
            self.visible = true;
            return;
        }

        self.visible = !self.visible;
        cx.notify();

        // Schedule the next blink
        let epoch = self.next_epoch();
        self._task = cx.spawn(async move |this, cx| {
            Timer::after(INTERVAL).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.blink(epoch, cx)).ok();
            }
        });
    }

    pub fn visible(&self) -> bool {
        // Keep showing the cursor if paused
        self.paused || self.visible
    }

    /// Pause the blinking, and delay 500ms to resume the blinking.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        self.last_pause = Instant::now();
        if self.paused {
            return;
        }
        let visible = self.visible();
        self.paused = true;
        self.visible = true;
        if !visible {
            cx.notify();
        }
        let epoch = self.next_epoch();
        self._task = cx.spawn(async move |this, cx| {
            loop {
                let remaining = this
                    .read_with(cx, |this, _| {
                        PAUSE_DELAY.saturating_sub(this.last_pause.elapsed())
                    })
                    .ok();
                let Some(remaining) = remaining else { return };
                if remaining.is_zero() {
                    break;
                }
                Timer::after(remaining).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.paused = false;
                this.blink(epoch, cx);
            });
        });
    }
}
