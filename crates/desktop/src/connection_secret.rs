//! Password intent is separate from the empty visual input field.
use turbodbn_services::{DatabaseKind, mysql::Profile};
#[derive(Clone, PartialEq)]
struct Scope {
    kind: DatabaseKind,
    host: String,
    port: u16,
    user: String,
}
impl From<&Profile> for Scope {
    fn from(p: &Profile) -> Self {
        Self {
            kind: p.kind,
            host: p.host.trim().to_ascii_lowercase(),
            port: p.port,
            user: p.user.clone(),
        }
    }
}
#[derive(Clone)]
pub struct Cached {
    scope: Scope,
    value: String,
}
impl Cached {
    pub fn new(profile: &Profile, value: String) -> Self {
        Self {
            scope: profile.into(),
            value,
        }
    }
    pub fn value_for(&self, profile: &Profile) -> Option<String> {
        self.matches(profile).then(|| self.value.clone())
    }
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }
    pub fn matches(&self, profile: &Profile) -> bool {
        self.scope == Scope::from(profile)
    }
}
pub fn same_target(a: &Profile, b: &Profile) -> bool {
    Scope::from(a) == Scope::from(b)
}
pub enum Input {
    Value(String),
    Stored,
}
pub fn input(
    profile: &Profile,
    saved: Option<&Profile>,
    cached: Option<&Cached>,
    entered: String,
    clear: bool,
) -> Result<Input, &'static str> {
    if clear {
        return Ok(Input::Value(String::new()));
    }
    if !entered.is_empty() {
        return Ok(Input::Value(entered));
    }
    if let Some(saved) = saved
        && saved.remember_password
    {
        if Scope::from(saved) != Scope::from(profile) {
            return Err("feedback.password.changed_target");
        }
        return Ok(Input::Stored);
    }
    if let Some(secret) = cached
        && secret.matches(profile)
    {
        return Ok(Input::Value(secret.value.clone()));
    }
    if cached.is_some() {
        return Err("feedback.password.changed_target");
    }
    Ok(Input::Value(String::new()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_secret_is_bound_to_server_and_user_not_display_name() {
        let p = Profile::default();
        let cached = Cached::new(&p, String::from_iter(['x'; 9]));
        let mut renamed = p.clone();
        renamed.name = "new name".into();
        renamed.database = "other_db".into();
        assert!(
            matches!(input(&renamed,Some(&p),Some(&cached),String::new(),false),Ok(Input::Value(s))if !s.is_empty())
        );
        renamed.host = "different-server".into();
        assert!(input(&renamed, Some(&p), Some(&cached), String::new(), false).is_err());
        assert!(
            matches!(input(&renamed,Some(&p),Some(&cached),String::new(),true),Ok(Input::Value(s))if s.is_empty())
        );
    }
    #[test]
    fn blank_means_keep_stored_and_explicit_clear_means_empty() {
        let p = Profile {
            remember_password: true,
            ..Default::default()
        };
        assert!(matches!(
            input(&p, Some(&p), None, String::new(), false),
            Ok(Input::Stored)
        ));
        assert!(
            matches!(input(&p,Some(&p),None,String::new(),true),Ok(Input::Value(s))if s.is_empty())
        );
    }
}
