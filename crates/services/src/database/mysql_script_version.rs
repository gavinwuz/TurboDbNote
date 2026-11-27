//! Numeric server-version checks; no SQL execution or global setting changes.
fn number(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor, patch))
}
pub(super) fn check(header: &str, target: &str) -> Result<(), String> {
    if target.to_ascii_lowercase().contains("mariadb") {
        return Err("mysql.script.version_family".into());
    }
    let target = number(target).ok_or("mysql.script.version_unknown")?;
    for line in header.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if !line.starts_with("--") {
            continue;
        }
        let Some(source) = line.strip_prefix("-- Server version") else {
            continue;
        };
        let source = number(source.trim()).ok_or("mysql.script.version_unknown")?;
        if target < source {
            return Err("mysql.script.downgrade".into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn numeric_versions_and_suffixes() {
        assert!(super::check("-- Server version 5.7.44-log", "5.7.44-log").is_ok());
        assert!(super::check("-- Server version 5.7.44", "8.0.40").is_ok());
        assert!(super::check("-- Server version 5.7.44", "5.7.9").is_err());
        assert!(super::check("-- Server version 5.7.44", "4.1.21-community-nt").is_err());
        assert!(super::check("-- Server version unknown", "5.7.44").is_err());
    }
}
