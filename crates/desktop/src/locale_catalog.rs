//! Shared by build-time validation and the runtime language loader.
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub type Strings = BTreeMap<String, String>;
#[derive(Clone, Deserialize)]
pub struct LanguagePack {
    #[serde(default = "legacy_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "unique_strings")]
    pub strings: Strings,
}
fn legacy_version() -> u32 {
    1
}
fn unique_strings<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Strings, D::Error> {
    struct Unique;
    impl<'de> Visitor<'de> for Unique {
        type Value = Strings;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string map without duplicate keys")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Strings, M::Error> {
            let mut strings = Strings::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if strings.insert(key, value).is_some() {
                    return Err(de::Error::custom("duplicate translation key"));
                }
            }
            Ok(strings)
        }
    }
    deserializer.deserialize_map(Unique)
}
pub fn parse_pack(json: &str) -> Result<LanguagePack, String> {
    let pack: LanguagePack = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if !(1..=2).contains(&pack.schema_version)
        || pack.id.trim().is_empty()
        || pack.name.trim().is_empty()
    {
        return Err("unsupported language pack version or missing metadata".into());
    }
    Ok(pack)
}
pub fn parse_aliases(json: &str) -> Result<Strings, String> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let aliases = unique_strings(&mut deserializer).map_err(|e| e.to_string())?;
    deserializer.end().map_err(|e| e.to_string())?;
    Ok(aliases)
}
pub fn semantic_key(key: &str) -> bool {
    let parts = key.split('.').collect::<Vec<_>>();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}
/// Named placeholders, with {{ and }} representing literal braces.
pub fn placeholders(text: &str) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                continue;
            }
            let mut name = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '}' {
                    closed = true;
                    break;
                }
                name.push(ch);
            }
            if !closed
                || name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || name.as_bytes()[0].is_ascii_digit()
            {
                return Err("invalid named placeholder".into());
            }
            names.insert(name);
        } else if ch == '}' && chars.next_if_eq(&'}').is_none() {
            return Err("unescaped closing brace".into());
        }
    }
    Ok(names)
}
pub fn format_message(template: &str, args: &[(&str, &str)]) -> Result<String, String> {
    let expected = placeholders(template)?;
    let provided = args
        .iter()
        .map(|(key, _)| key.to_string())
        .collect::<BTreeSet<_>>();
    if expected != provided || provided.len() != args.len() {
        return Err("translation argument mismatch".into());
    }
    let mut result = String::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            if chars.next_if_eq(&'{').is_some() {
                result.push('{');
                continue;
            }
            let name = chars
                .by_ref()
                .take_while(|ch| *ch != '}')
                .collect::<String>();
            result.push_str(
                args.iter()
                    .find(|(key, _)| *key == name)
                    .expect("validated argument")
                    .1,
            );
        } else if ch == '}' {
            chars.next();
            result.push('}');
        } else {
            result.push(ch);
        }
    }
    Ok(result)
}
pub fn validate_catalog(
    english: &LanguagePack,
    other: &LanguagePack,
    aliases: &Strings,
) -> Result<(), String> {
    if english.strings.keys().collect::<Vec<_>>() != other.strings.keys().collect::<Vec<_>>() {
        return Err("language key sets differ".into());
    }
    for (key, value) in &english.strings {
        if !semantic_key(key) {
            return Err(format!("invalid semantic key: {key}"));
        }
        if value.trim().is_empty() || other.strings[key].trim().is_empty() {
            return Err(format!("empty translation: {key}"));
        }
        if placeholders(value)? != placeholders(&other.strings[key])? {
            return Err(format!("placeholder mismatch: {key}"));
        }
    }
    for key in aliases.values() {
        if !english.strings.contains_key(key) {
            return Err(format!("alias target missing: {key}"));
        }
    }
    Ok(())
}
pub fn normalize_external(
    pack: LanguagePack,
    english: &LanguagePack,
    aliases: &Strings,
) -> Result<LanguagePack, String> {
    let mut strings = Strings::new();
    // Legacy entries are applied first; an explicit semantic key always wins.
    for (old, key) in aliases {
        if let Some(value) = pack.strings.get(old) {
            strings.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in &pack.strings {
        if english.strings.contains_key(key) {
            strings.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in &strings {
        if value.trim().is_empty() || placeholders(value)? != placeholders(&english.strings[key])? {
            return Err(format!("invalid external translation: {key}"));
        }
    }
    Ok(LanguagePack { strings, ..pack })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pack(strings: &str) -> LanguagePack {
        parse_pack(&format!(
            r#"{{"id":"test","name":"Test","strings":{strings}}}"#
        ))
        .unwrap()
    }
    #[test]
    fn rejects_duplicate_keys_and_invalid_placeholders() {
        assert!(
            parse_pack(r#"{"id":"en","name":"English","strings":{"a.b":"A","a.b":"B"}}"#).is_err()
        );
        assert!(placeholders("Bad {count").is_err());
        assert!(placeholders("Bad {0}").is_err());
        assert!(placeholders("Bad }").is_err());
    }
    #[test]
    fn arguments_are_inserted_once_without_reinterpreting_user_text() {
        assert_eq!(
            format_message("{{{count}}} {name}", &[("count", "2"), ("name", "{count}")]).unwrap(),
            "{2} {count}"
        );
        assert!(format_message("{count}", &[]).is_err());
        assert!(format_message("{count}", &[("count", "1"), ("count", "2")]).is_err());
    }
    #[test]
    fn validates_catalog_parity_and_external_legacy_precedence() {
        let en = pack(r#"{"panel.inspector.title":"Inspector","chat.count":"{count} models"}"#);
        let zh = pack(r#"{"panel.inspector.title":"检查器","chat.count":"{count} 个模型"}"#);
        let aliases = Strings::from([("检查器".into(), "panel.inspector.title".into())]);
        validate_catalog(&en, &zh, &aliases).unwrap();
        assert!(
            validate_catalog(&en, &pack(r#"{"panel.inspector.title":"A"}"#), &aliases).is_err()
        );
        let external = normalize_external(
            pack(r#"{"检查器":"旧覆盖","panel.inspector.title":"新覆盖"}"#),
            &en,
            &aliases,
        )
        .unwrap();
        assert_eq!(external.strings["panel.inspector.title"], "新覆盖");
        assert!(normalize_external(pack(r#"{"chat.count":"{wrong}"}"#), &en, &aliases).is_err());
    }
}
