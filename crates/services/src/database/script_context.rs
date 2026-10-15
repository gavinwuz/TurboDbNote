//! Narrow MySQL dump grammar. Ordinary Note SQL never uses these capabilities.
use crate::DatabaseKind;
use sqlparser::{
    dialect::MySqlDialect,
    tokenizer::{Token, Tokenizer},
};

pub const MAX_BYTES: usize = 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Use(String),
    Session,
    Variable,
    Isolation,
    Lock,
    Unlock,
    Begin,
    Commit,
    Rollback,
}
impl Control {
    pub fn permission(&self) -> &'static str {
        match self {
            Self::Lock | Self::Unlock => "script_lock",
            Self::Variable => "script_variables",
            Self::Begin | Self::Commit | Self::Rollback | Self::Isolation => "script_transaction",
            _ => "script_session",
        }
    }
    pub fn confirm(&self) -> bool {
        !matches!(self, Self::Use(_) | Self::Rollback)
    }
}
const VARIABLES: &[&str] = &[
    "CHARACTER_SET_CLIENT",
    "CHARACTER_SET_RESULTS",
    "CHARACTER_SET_CONNECTION",
    "COLLATION_CONNECTION",
    "FOREIGN_KEY_CHECKS",
    "UNIQUE_CHECKS",
    "SQL_MODE",
    "TIME_ZONE",
    "AUTOCOMMIT",
    "SQL_NOTES",
];
fn tokens(sql: &str) -> Result<Vec<Token>, String> {
    if sql.len() > MAX_BYTES || sql.contains('\0') {
        return Err("script.error.statement_size".into());
    }
    let mut tokens = Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize()
        .map_err(|_| "script.error.syntax")?
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    if tokens.last() == Some(&Token::SemiColon) {
        tokens.pop();
    }
    if tokens.contains(&Token::SemiColon) {
        return Err("script.error.syntax".into());
    }
    Ok(tokens)
}
fn word(t: &Token) -> Option<String> {
    match t {
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
        _ => None,
    }
}
fn ident(t: &Token) -> Option<String> {
    match t {
        Token::Word(w) if w.quote_style.is_none() || w.quote_style == Some('`') => {
            Some(w.value.clone())
        }
        _ => None,
    }
}
/// Strict, script-only DROP TRIGGER grammar; never accepts a second statement.
pub fn drop_trigger_target(sql: &str) -> Result<Option<Vec<String>>, String> {
    let ts = tokens(sql)?;
    if ts.len() < 2
        || word(&ts[0]).as_deref() != Some("DROP")
        || word(&ts[1]).as_deref() != Some("TRIGGER")
    {
        return Ok(None);
    }
    let target = if ts.get(2).and_then(word).as_deref() == Some("IF") {
        if ts.get(3).and_then(word).as_deref() != Some("EXISTS") {
            return Err("script.error.syntax".into());
        }
        &ts[4..]
    } else {
        &ts[2..]
    };
    let parts = match target {
        [name] => vec![ident(name).ok_or("script.error.syntax")?],
        [schema, Token::Period, name] => vec![
            ident(schema).ok_or("script.error.syntax")?,
            ident(name).ok_or("script.error.syntax")?,
        ],
        _ => return Err("script.error.syntax".into()),
    };
    if parts
        .iter()
        .any(|s| s.is_empty() || s.len() > 64 || s.chars().any(char::is_control))
    {
        return Err("script.error.syntax".into());
    }
    Ok(Some(parts))
}
pub fn keys_target(sql: &str) -> Result<Option<Vec<String>>, String> {
    let ts = tokens(sql)?;
    if ts.len() < 5
        || word(&ts[0]).as_deref() != Some("ALTER")
        || word(&ts[1]).as_deref() != Some("TABLE")
    {
        return Ok(None);
    }
    let n = ts.len();
    if !matches!(word(&ts[n - 2]).as_deref(), Some("ENABLE" | "DISABLE"))
        || word(&ts[n - 1]).as_deref() != Some("KEYS")
    {
        return Ok(None);
    }
    let target = &ts[2..n - 2];
    let parts = if target.len() == 1 {
        vec![ident(&target[0]).ok_or("script.error.syntax")?]
    } else if target.len() == 3 && target[1] == Token::Period {
        vec![
            ident(&target[0]).ok_or("script.error.syntax")?,
            ident(&target[2]).ok_or("script.error.syntax")?,
        ]
    } else {
        return Err("script.error.syntax".into());
    };
    Ok(Some(parts))
}
pub fn relaxes_checks(sql: &str) -> Result<bool, String> {
    let ts = tokens(sql)?;
    for row in ts.split(|t| *t == Token::Comma) {
        if let Some(eq) = row.iter().position(|t| *t == Token::Eq)
            && row[..eq].iter().any(|t| {
                matches!(
                    word(t).as_deref(),
                    Some("FOREIGN_KEY_CHECKS" | "UNIQUE_CHECKS")
                )
            })
            && row.get(eq + 1).and_then(value).as_deref() == Some("0")
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn value(t: &Token) -> Option<String> {
    match t {
        Token::SingleQuotedString(s) => Some(s.clone()),
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.clone()),
        Token::Number(n, _) => Some(n.clone()),
        _ => None,
    }
}
fn reference(ts: &[Token]) -> Option<(bool, String)> {
    let text = ts
        .iter()
        .map(ToString::to_string)
        .collect::<String>()
        .to_ascii_uppercase();
    let saved = match text.as_str() {
        "@SAVED_CS_CLIENT" => Some("CHARACTER_SET_CLIENT"),
        "@SAVED_CS_RESULTS" => Some("CHARACTER_SET_RESULTS"),
        "@SAVED_COL_CONNECTION" => Some("COLLATION_CONNECTION"),
        "@SAVED_SQL_MODE" => Some("SQL_MODE"),
        "@SAVED_TIME_ZONE" => Some("TIME_ZONE"),
        _ => None,
    };
    if let Some(name) = saved {
        return Some((true, name.into()));
    }
    if let Some(name) = text.strip_prefix("@@") {
        let name = name.strip_prefix("SESSION.").unwrap_or(name);
        return VARIABLES.contains(&name).then(|| (false, name.into()));
    }
    text.strip_prefix("@OLD_")
        .filter(|n| VARIABLES.contains(n))
        .map(|n| (true, n.into()))
}
fn literal(name: &str, ts: &[Token]) -> bool {
    if ts.len() != 1 {
        return false;
    }
    let Some(v) = value(&ts[0]) else { return false };
    let upper = v.to_ascii_uppercase();
    match name {
        "FOREIGN_KEY_CHECKS" | "UNIQUE_CHECKS" | "AUTOCOMMIT" | "SQL_NOTES" => {
            matches!(upper.as_str(), "0" | "1")
        }
        "CHARACTER_SET_CLIENT" | "CHARACTER_SET_RESULTS" | "CHARACTER_SET_CONNECTION" => {
            matches!(upper.as_str(), "UTF8" | "UTF8MB3" | "UTF8MB4")
                || name == "CHARACTER_SET_RESULTS" && upper == "NULL"
        }
        "COLLATION_CONNECTION" => {
            upper.starts_with("UTF8")
                && upper
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        }
        "TIME_ZONE" => {
            upper == "SYSTEM"
                || v.is_ascii()
                    && v.len() == 6
                    && matches!(v.as_bytes()[0], b'+' | b'-')
                    && v.as_bytes()[3] == b':'
                    && v[1..3].parse::<u8>().is_ok_and(|h| h <= 14)
                    && v[4..6].parse::<u8>().is_ok_and(|m| m < 60)
        }
        "SQL_MODE" => upper.split(',').all(|m| {
            matches!(
                m,
                "" | "NO_AUTO_VALUE_ON_ZERO"
                    | "STRICT_TRANS_TABLES"
                    | "STRICT_ALL_TABLES"
                    | "NO_ZERO_IN_DATE"
                    | "NO_ZERO_DATE"
                    | "ERROR_FOR_DIVISION_BY_ZERO"
                    | "NO_ENGINE_SUBSTITUTION"
                    | "ONLY_FULL_GROUP_BY"
                    | "PAD_CHAR_TO_FULL_LENGTH"
            )
        }),
        _ => false,
    }
}
fn user_variable(ts: &[Token]) -> bool {
    let text = ts.iter().map(ToString::to_string).collect::<String>();
    text.strip_prefix('@').is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !name.as_bytes()[0].is_ascii_digit()
    })
}
/// Produce inspection-only SQL; execution always uses the original frozen statement.
pub fn variable_select(sql: &str) -> Result<Option<(String, bool)>, String> {
    let mut ts = tokens(sql)?;
    if ts.first().and_then(word).as_deref() != Some("SELECT") {
        return Ok(None);
    }
    let Some(into) = ts.iter().position(|t| word(t).as_deref() == Some("INTO")) else {
        return Ok(None);
    };
    let from = ts
        .iter()
        .enumerate()
        .skip(into + 1)
        .find(|(_, t)| word(t).as_deref() == Some("FROM"))
        .map(|(i, _)| i)
        .ok_or("script.error.control")?;
    if !user_variable(&ts[into + 1..from]) {
        return Err("script.error.control".into());
    }
    ts.drain(into..from);
    let n = ts.len();
    let locked = n >= 2
        && word(&ts[n - 2]).as_deref() == Some("FOR")
        && word(&ts[n - 1]).as_deref() == Some("UPDATE");
    if locked {
        ts.truncate(n - 2);
    }
    Ok(Some((
        ts.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" "),
        locked,
    )))
}
pub fn control(sql: &str) -> Result<Option<Control>, String> {
    let ts = tokens(sql)?;
    let words = ts
        .iter()
        .map(|t| word(t).unwrap_or_default())
        .collect::<Vec<_>>();
    let first = words.first().map(String::as_str).unwrap_or_default();
    let bad = || Err("script.error.control".into());
    match first {
        "USE" => {
            if ts.len() == 2 {
                ident(&ts[1])
                    .map(|v| Some(Control::Use(v)))
                    .ok_or("script.error.control".into())
            } else {
                bad()
            }
        }
        "SET" => {
            let isolation = words.get(1..).unwrap_or_default();
            let isolation = isolation
                .strip_prefix(&["SESSION".into()])
                .unwrap_or(isolation);
            if isolation.starts_with(&["TRANSACTION".into(), "ISOLATION".into(), "LEVEL".into()]) {
                if matches!(
                    isolation[3..]
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .as_slice(),
                    ["REPEATABLE", "READ"]
                        | ["READ", "COMMITTED"]
                        | ["READ", "UNCOMMITTED"]
                        | ["SERIALIZABLE"]
                ) {
                    return Ok(Some(Control::Isolation));
                }
                return bad();
            }
            if let Some(eq) = ts.iter().position(|t| *t == Token::Eq)
                && user_variable(&ts[1..eq])
                && reference(&ts[1..eq]).is_none()
            {
                let rhs = &ts[eq + 1..];
                let numeric = |t: &Token| matches!(t,Token::Number(n,_) if n.bytes().all(|b|b.is_ascii_digit()) && n.len()<=20);
                if matches!(rhs,[number] if numeric(number))
                    || rhs.len() >= 3
                        && matches!(rhs[rhs.len() - 2], Token::Plus | Token::Minus)
                        && numeric(&rhs[rhs.len() - 1])
                        && user_variable(&rhs[..rhs.len() - 2])
                {
                    return Ok(Some(Control::Variable));
                }
                return bad();
            }
            if words.get(1).map(String::as_str) == Some("NAMES")
                || words.get(1..3) == Some(&["CHARACTER".into(), "SET".into()])
            {
                let start = if words[1] == "NAMES" { 2 } else { 3 };
                if ts.len() == start + 1 && literal("CHARACTER_SET_CLIENT", &ts[start..]) {
                    return Ok(Some(Control::Session));
                }
                if ts.len() == start + 3
                    && words[start + 1] == "COLLATE"
                    && literal("CHARACTER_SET_CLIENT", &ts[start..start + 1])
                    && literal("COLLATION_CONNECTION", &ts[start + 2..])
                {
                    return Ok(Some(Control::Session));
                }
                return bad();
            }
            let start = if words.get(1).map(String::as_str) == Some("SESSION") {
                2
            } else {
                1
            };
            for assignment in ts[start..].split(|t| *t == Token::Comma) {
                let Some(eq) = assignment.iter().position(|t| *t == Token::Eq) else {
                    return bad();
                };
                let left = &assignment[..eq];
                let right = &assignment[eq + 1..];
                if let Some((true, name)) = reference(left) {
                    if reference(right) != Some((false, name)) {
                        return bad();
                    }
                } else {
                    let name = if left.len() == 1 {
                        word(&left[0])
                    } else {
                        reference(left).and_then(|(old, n)| (!old).then_some(n))
                    };
                    let Some(name) = name.filter(|n| VARIABLES.contains(&n.as_str())) else {
                        return bad();
                    };
                    if !literal(&name, right) && reference(right) != Some((true, name)) {
                        return bad();
                    }
                }
            }
            if ts.len() <= start {
                return bad();
            }
            Ok(Some(Control::Session))
        }
        "LOCK" => {
            if words.get(1).map(String::as_str) != Some("TABLES") {
                return bad();
            }
            for table in ts[2..].split(|t| *t == Token::Comma) {
                if table.len() < 2 {
                    return bad();
                }
                let end = table.len() - 1;
                if !matches!(word(&table[end]).as_deref(), Some("READ" | "WRITE")) {
                    return bad();
                }
                let name = &table[..end];
                if !(name.len() == 1 && ident(&name[0]).is_some()
                    || name.len() == 3
                        && ident(&name[0]).is_some()
                        && name[1] == Token::Period
                        && ident(&name[2]).is_some())
                {
                    return bad();
                }
            }
            if ts.len() < 4 {
                return bad();
            }
            Ok(Some(Control::Lock))
        }
        "UNLOCK" => {
            if words == ["UNLOCK", "TABLES"] {
                Ok(Some(Control::Unlock))
            } else {
                bad()
            }
        }
        "START" => {
            if words == ["START", "TRANSACTION"] {
                Ok(Some(Control::Begin))
            } else {
                bad()
            }
        }
        "BEGIN" => {
            if words == ["BEGIN"] || words == ["BEGIN", "WORK"] {
                Ok(Some(Control::Begin))
            } else {
                bad()
            }
        }
        "COMMIT" => {
            if words == ["COMMIT"] || words == ["COMMIT", "WORK"] {
                Ok(Some(Control::Commit))
            } else {
                bad()
            }
        }
        "ROLLBACK" => {
            if words == ["ROLLBACK"] || words == ["ROLLBACK", "WORK"] {
                Ok(Some(Control::Rollback))
            } else {
                bad()
            }
        }
        _ => Ok(None),
    }
}
/// Expand executable comments outside literals; version guards are applied only at execution.
pub fn expand(sql: &str, version: Option<u32>) -> Result<String, String> {
    if sql.len() > MAX_BYTES || sql.contains('\0') {
        return Err("script.error.statement_size".into());
    }
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let mut quote = None;
    let mut line = false;
    while i < b.len() {
        if line {
            let start = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            out.push_str(&sql[start..i]);
            line = false;
            continue;
        }
        if let Some(q) = quote {
            let start = i;
            i += 1;
            if b[start] == b'\\' {
                if q == b'`' || i == b.len() {
                    return Err("script.error.escaping".into());
                }
                i += 1;
                while i < b.len() && !sql.is_char_boundary(i) {
                    i += 1;
                }
                out.push_str(&sql[start..i]);
                continue;
            }
            if b[start] == q {
                if b.get(i) == Some(&q) {
                    i += 1;
                } else {
                    quote = None;
                }
            }
            while i < b.len() && !sql.is_char_boundary(i) {
                i += 1;
            }
            out.push_str(&sql[start..i]);
            continue;
        }
        if matches!(b[i], b'\'' | b'"' | b'`') {
            quote = Some(b[i]);
            out.push(b[i] as char);
            i += 1;
            continue;
        }
        if b[i] == b'#'
            || b.get(i..i + 2) == Some(b"--")
                && b.get(i + 2).is_none_or(|c| c.is_ascii_whitespace())
        {
            line = true;
            continue;
        }
        if b.get(i..i + 2) == Some(b"/*") {
            let end = sql[i + 2..]
                .find("*/")
                .map(|n| i + 2 + n)
                .ok_or("script.error.unterminated")?;
            if b.get(i + 2) == Some(&b'!') {
                let mut start = i + 3;
                while start < end && b[start].is_ascii_digit() {
                    start += 1;
                }
                let gate = if start > i + 3 {
                    sql[i + 3..start]
                        .parse::<u32>()
                        .map_err(|_| "script.error.control")?
                } else {
                    0
                };
                if version.is_none_or(|v| v >= gate) {
                    let body = &sql[start..end];
                    if body.contains("/*") {
                        return Err("script.error.control".into());
                    }
                    out.push(' ');
                    out.push_str(body);
                    out.push(' ');
                }
            } else {
                out.push_str(&sql[i..end + 2]);
            }
            i = end + 2;
            continue;
        }
        let start = i;
        i += 1;
        while i < b.len() && !sql.is_char_boundary(i) {
            i += 1;
        }
        out.push_str(&sql[start..i]);
    }
    if quote.is_some() {
        return Err("script.error.unterminated".into());
    }
    Ok(out)
}
pub fn version_number(version: &str) -> Result<u32, String> {
    if version.to_ascii_lowercase().contains("mariadb") {
        return Err("script.error.legacy".into());
    }
    let mut numbers = version.split('.').take(3).map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
    });
    let major = numbers
        .next()
        .ok_or("script.error.legacy")?
        .map_err(|_| "script.error.legacy")?;
    let minor = numbers
        .next()
        .ok_or("script.error.legacy")?
        .map_err(|_| "script.error.legacy")?;
    let patch = numbers
        .next()
        .ok_or("script.error.legacy")?
        .map_err(|_| "script.error.legacy")?;
    if major > 99 || minor > 99 || patch > 99 {
        return Err("script.error.legacy".into());
    }
    major
        .checked_mul(10000)
        .and_then(|n| n.checked_add(minor * 100))
        .and_then(|n| n.checked_add(patch))
        .ok_or("script.error.legacy".into())
}
pub fn validate_target(kind: DatabaseKind, sql: &str, database: &str) -> Result<(), String> {
    if kind != DatabaseKind::MySql {
        return Ok(());
    }
    let expanded = expand(sql, None)?;
    if let Some(Control::Use(target)) = control(&expanded)?
        && target != database
    {
        return Err("script.error.database_mismatch".into());
    }
    // Fully qualified names must also stay within the explicitly selected database.
    let ts = tokens(&expanded)?;
    for window in ts.windows(3) {
        if window[1] == Token::Period
            && let Some(name) = ident(&window[0])
            && let Some(_) = ident(&window[2])
            && name != database
        {
            // Table aliases in queries are handled by the ordinary query validator, not guessed here.
            if matches!(control(&expanded)?, Some(Control::Lock)) {
                return Err("script.error.database_mismatch".into());
            }
        }
    }
    Ok(())
}
#[derive(Default)]
pub struct Sequence {
    locked: bool,
    transaction: bool,
    autocommit_off: bool,
    pending_write: bool,
    saved_autocommit: Option<bool>,
}
impl Sequence {
    pub fn push(&mut self, kind: DatabaseKind, sql: &str, write: bool) -> Result<(), String> {
        if kind != DatabaseKind::MySql {
            return Ok(());
        }
        let expanded = expand(sql, None)?;
        match control(&expanded)? {
            Some(Control::Lock) => {
                if self.locked {
                    return Err("script.error.lock_sequence".into());
                }
                self.locked = true;
                self.transaction = false;
                self.pending_write = false;
            }
            Some(Control::Unlock) => {
                if self.locked {
                    self.pending_write = false;
                    self.transaction = false;
                }
                self.locked = false;
            }
            Some(Control::Begin) => {
                if self.locked || self.transaction {
                    return Err("script.error.lock_sequence".into());
                }
                self.transaction = true;
            }
            Some(Control::Commit | Control::Rollback) => {
                self.transaction = false;
                self.pending_write = false;
            }
            Some(Control::Session) => {
                let ts = tokens(&expanded)?;
                for assignment in ts[1..].split(|t| *t == Token::Comma) {
                    if let Some(eq) = assignment.iter().position(|t| *t == Token::Eq)
                        && reference(&assignment[..eq]) == Some((true, "AUTOCOMMIT".into()))
                    {
                        self.saved_autocommit = Some(self.autocommit_off);
                    }
                    if let Some(eq) = assignment.iter().position(|t| *t == Token::Eq)
                        && assignment[..eq]
                            .iter()
                            .any(|t| word(t).as_deref() == Some("AUTOCOMMIT"))
                    {
                        self.autocommit_off = if reference(&assignment[eq + 1..])
                            == Some((true, "AUTOCOMMIT".into()))
                        {
                            self.saved_autocommit
                                .ok_or("script.error.transaction_sequence")?
                        } else if assignment[eq + 1..].len() == 1 {
                            value(&assignment[eq + 1]).as_deref() == Some("0")
                        } else {
                            return Err("script.error.transaction_sequence".into());
                        };
                        if !self.autocommit_off {
                            self.pending_write = false;
                            self.transaction = false;
                        }
                    }
                }
            }
            None if write => {
                self.pending_write |= self.transaction || self.autocommit_off;
            }
            _ => {}
        }
        Ok(())
    }
    pub fn finish(&self) -> Result<(), String> {
        if self.locked {
            return Err("script.error.lock_sequence".into());
        }
        if self.transaction || self.pending_write {
            return Err("script.error.transaction_sequence".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locks_and_transactions_require_explicit_completion() {
        let kind = DatabaseKind::MySql;
        let mut seq = Sequence::default();
        seq.push(kind, "LOCK TABLES t WRITE", true).unwrap();
        assert!(seq.finish().is_err());
        assert!(seq.push(kind, "BEGIN", true).is_err());
        seq.push(kind, "UNLOCK TABLES", false).unwrap();
        assert!(seq.finish().is_ok());
        let mut seq = Sequence::default();
        seq.push(kind, "SET autocommit=0", true).unwrap();
        seq.push(kind, "INSERT INTO t VALUES(1)", true).unwrap();
        assert!(seq.finish().is_err());
        seq.push(kind, "COMMIT", true).unwrap();
        assert!(seq.finish().is_ok());
        let mut seq = Sequence::default();
        for sql in [
            "SET @OLD_AUTOCOMMIT=@@AUTOCOMMIT",
            "SET autocommit=0",
            "INSERT INTO t VALUES(1)",
            "COMMIT",
            "SET autocommit=@OLD_AUTOCOMMIT",
        ] {
            seq.push(kind, sql, true).unwrap();
        }
        assert!(seq.finish().is_ok());
        let mut seq = Sequence::default();
        seq.push(kind, "BEGIN", true).unwrap();
        seq.push(kind, "UNLOCK TABLES", false).unwrap();
        assert!(seq.finish().is_err());
    }
    #[test]
    fn dump_controls_are_narrow_and_global_mutations_are_rejected() {
        for sql in [
            "USE `creation_zone`;",
            "SET NAMES utf8mb4;",
            "SET FOREIGN_KEY_CHECKS=0;",
            "SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0;",
            "SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS;",
            "SET @saved_cs_client=@@character_set_client;",
            "SET character_set_client=@saved_cs_client;",
            "SET @OLD_SQL_NOTES=@@SQL_NOTES, SQL_NOTES=0;",
            "LOCK TABLES `t` WRITE;",
            "UNLOCK TABLES;",
            "SET autocommit=0;",
            "START TRANSACTION;",
            "COMMIT;",
        ] {
            assert!(control(sql).unwrap().is_some());
        }
        for sql in [
            "SET GLOBAL FOREIGN_KEY_CHECKS=0",
            "SET @@GLOBAL.SQL_MODE=''",
            "SET SQL_LOG_BIN=0",
            "SET SQL_MODE='ANSI_QUOTES'",
            "SET NAMES latin1",
            "SET @OLD_SQL_MODE=(SELECT 1)",
            "LOCK INSTANCE FOR BACKUP",
            "LOCK TABLES t WRITE; DROP TABLE t",
        ] {
            assert!(control(sql).is_err());
        }
    }
    #[test]
    fn executable_comments_keep_literals_and_apply_version_guards() {
        assert!(
            expand("/*!40101 SET NAMES utf8mb4 */;", Some(80405))
                .unwrap()
                .contains("SET NAMES")
        );
        assert_eq!(
            expand("/*!99999 SET NAMES utf8mb4 */;", Some(80405)).unwrap(),
            ";"
        );
        assert_eq!(
            expand("SELECT '/*!40101 DROP TABLE t */';", Some(80405)).unwrap(),
            "SELECT '/*!40101 DROP TABLE t */';"
        );
        assert!(validate_target(DatabaseKind::MySql, "USE other", "current").is_err());
    }
}
