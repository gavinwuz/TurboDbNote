//! Application defaults are resolved once per new session, never written over saved profiles.
use std::sync::{OnceLock, RwLock};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Defaults {
    pub connect: u64,
    pub execution: u64,
}
impl Default for Defaults {
    fn default() -> Self {
        Self {
            connect: 5,
            execution: turbodbn_core::timeouts::DEFAULT_EXECUTION_SECONDS,
        }
    }
}
impl Defaults {
    pub fn valid(self) -> bool {
        (1..=60).contains(&self.connect)
            && (1..=turbodbn_core::timeouts::MAX_EXECUTION_SECONDS).contains(&self.execution)
    }
}
fn state() -> &'static RwLock<Defaults> {
    static STATE: OnceLock<RwLock<Defaults>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(Defaults::default()))
}
pub fn configure(defaults: Defaults) -> Result<(), &'static str> {
    if !defaults.valid() {
        return Err("database.timeout.defaults_invalid");
    }
    *state()
        .write()
        .map_err(|_| "database.timeout.defaults_invalid")? = defaults;
    Ok(())
}
pub fn current() -> Defaults {
    state().read().map(|v| *v).unwrap_or_default()
}
impl crate::mysql::Profile {
    pub fn resolve_timeouts(&self) -> Self {
        self.with_timeout_defaults(current())
    }
    pub fn with_timeout_defaults(&self, defaults: Defaults) -> Self {
        let mut profile = self.clone();
        if profile.inherit_timeouts {
            profile.timeout_seconds = defaults.connect;
            profile.execution_timeout_seconds = defaults.execution;
        }
        profile.inherit_timeouts = false;
        profile
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inheritance_preserves_overrides_and_running_snapshots() {
        let mut p: crate::mysql::Profile =
            serde_json::from_str(r#"{"timeout_seconds":7,"execution_timeout_seconds":600}"#)
                .unwrap();
        assert!(!p.inherit_timeouts);
        let a = Defaults {
            connect: 5,
            execution: 300,
        };
        let b = Defaults {
            connect: 9,
            execution: 900,
        };
        assert_eq!(p.with_timeout_defaults(b).execution_timeout_seconds, 600);
        p.inherit_timeouts = true;
        let running = p.with_timeout_defaults(a);
        assert_eq!(
            running.with_timeout_defaults(b).execution_timeout_seconds,
            300
        );
        assert_eq!(p.with_timeout_defaults(b).execution_timeout_seconds, 900);
        assert_eq!(p.execution_timeout_seconds, 600);
        assert!(
            !Defaults {
                connect: 0,
                execution: 300
            }
            .valid()
        );
        assert!(
            !Defaults {
                connect: 5,
                execution: 0
            }
            .valid()
        );
    }
}
