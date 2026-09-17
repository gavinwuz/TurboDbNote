use std::collections::HashSet;
use turbodbn_services::agent::Engine;

/// One automatic attempt per engine/window lifetime. An explicit disconnect
/// also suppresses future automatic attempts; manual retry is independent.
#[derive(Default)]
pub struct AutoConnect {
    attempted: HashSet<Engine>,
    scheduled: Option<Engine>,
}
impl AutoConnect {
    pub fn schedule(
        &mut self,
        engine: Engine,
        enabled: bool,
        configured: bool,
        occupied: bool,
    ) -> bool {
        if !enabled
            || !configured
            || occupied
            || self.scheduled.is_some()
            || self.attempted.contains(&engine)
        {
            return false;
        }
        self.scheduled = Some(engine);
        true
    }
    pub fn take(&mut self, engine: Engine) -> bool {
        if self.scheduled != Some(engine) {
            return false;
        }
        self.scheduled = None;
        !self.attempted.contains(&engine)
    }
    pub fn suppress(&mut self, engine: Engine) {
        self.attempted.insert(engine);
        if self.scheduled == Some(engine) {
            self.scheduled = None;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_once_and_never_reconnect_after_manual_disconnect() {
        let mut policy = AutoConnect::default();
        assert!(policy.schedule(Engine::Codex, true, true, false));
        assert!(!policy.schedule(Engine::Codex, true, true, false));
        assert!(policy.take(Engine::Codex));
        policy.suppress(Engine::Codex);
        assert!(!policy.schedule(Engine::Codex, true, true, false));
        assert!(policy.schedule(Engine::Claude, true, true, false));
        policy.suppress(Engine::Claude);
        assert!(!policy.take(Engine::Claude));
    }
    #[test]
    fn disabled_unconfigured_and_busy_states_do_not_schedule() {
        let mut policy = AutoConnect::default();
        assert!(!policy.schedule(Engine::Codex, false, true, false));
        assert!(!policy.schedule(Engine::Codex, true, false, false));
        assert!(!policy.schedule(Engine::Codex, true, true, true));
        assert!(policy.schedule(Engine::Codex, true, true, false));
        assert!(!policy.take(Engine::Claude));
        assert!(policy.take(Engine::Codex));
    }
}
