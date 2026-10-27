//! Bounded lexical recovery of SHOW CREATE TABLE components; never executes SQL.
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Parts {
    pub columns: BTreeMap<String, String>,
    pub constraints: Vec<String>,
    pub options: String,
}

pub(crate) fn index_name(fragment: &str) -> Option<String> {
    use sqlparser::{
        dialect::MySqlDialect,
        tokenizer::{Token, Tokenizer},
    };
    let tokens = Tokenizer::new(&MySqlDialect {}, fragment).tokenize().ok()?;
    let words = tokens
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let word = |i: usize| match words.get(i) {
        Some(Token::Word(w)) => Some(w.value.clone()),
        _ => None,
    };
    let first = word(0)?.to_ascii_uppercase();
    if first == "PRIMARY" {
        return Some("PRIMARY".into());
    }
    let start = usize::from(matches!(first.as_str(), "UNIQUE" | "FULLTEXT" | "SPATIAL"));
    if matches!(word(start)?.to_ascii_uppercase().as_str(), "KEY" | "INDEX") {
        word(start + 1)
    } else {
        None
    }
}

pub(crate) fn split(ddl: &str) -> Option<Parts> {
    if ddl.len() > 256_000 {
        return None;
    }
    let bytes = ddl.as_bytes();
    let mut quote = 0;
    let mut comment = 0;
    let mut depth = 0usize;
    let mut start = None;
    let mut end = None;
    let mut fragments = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        if comment == 1 {
            if c == b'\n' {
                comment = 0;
            }
            i += 1;
            continue;
        }
        if comment == 2 {
            if c == b'*' && next == Some(b'/') {
                comment = 0;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if quote != 0 {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == quote {
                if next == Some(quote) {
                    i += 2;
                    continue;
                }
                quote = 0;
            }
            i += 1;
            continue;
        }
        if matches!(c, b'\'' | b'"' | b'`') {
            quote = c;
        } else if c == b'/' && next == Some(b'*') {
            comment = 2;
            i += 2;
            continue;
        } else if c == b'#'
            || c == b'-'
                && next == Some(b'-')
                && bytes.get(i + 2).is_none_or(u8::is_ascii_whitespace)
        {
            comment = 1;
        } else if c == b'(' {
            depth += 1;
            if depth == 1 && start.is_none() {
                start = Some(i + 1);
            }
        } else if c == b')' {
            depth = depth.checked_sub(1)?;
            if depth == 0 && end.is_none() {
                fragments.push(ddl.get(start?..i)?.trim().to_string());
                end = Some(i + 1);
            }
        } else if c == b',' && depth == 1 && end.is_none() {
            fragments.push(ddl.get(start?..i)?.trim().to_string());
            start = Some(i + 1);
        } else if c == b';'
            && (depth > 0 || bytes[i + 1..].iter().any(|b| !b.is_ascii_whitespace()))
        {
            return None;
        }
        i += 1;
    }
    if quote != 0 || comment == 2 || depth != 0 || fragments.len() > 2500 {
        return None;
    }
    let mut columns = BTreeMap::new();
    let mut constraints = Vec::new();
    for fragment in fragments {
        use sqlparser::{
            dialect::MySqlDialect,
            tokenizer::{Token, Tokenizer},
        };
        let tokens = Tokenizer::new(&MySqlDialect {}, &fragment)
            .tokenize()
            .ok()?;
        let Some(Token::Word(first)) = tokens.iter().find(|t| !matches!(t, Token::Whitespace(_)))
        else {
            return None;
        };
        if first.quote_style.is_none()
            && matches!(
                first.value.to_ascii_uppercase().as_str(),
                "PRIMARY"
                    | "UNIQUE"
                    | "KEY"
                    | "INDEX"
                    | "FULLTEXT"
                    | "SPATIAL"
                    | "CONSTRAINT"
                    | "FOREIGN"
                    | "CHECK"
            )
        {
            constraints.push(fragment);
        } else if columns.insert(first.value.clone(), fragment).is_some() {
            return None;
        }
    }
    if columns.is_empty() {
        return None;
    }
    Some(Parts {
        columns,
        constraints,
        options: ddl
            .get(end?..)?
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string(),
    })
}

pub(crate) fn options_key(options: &str) -> String {
    use sqlparser::{
        dialect::MySqlDialect,
        tokenizer::{Token, Tokenizer, Whitespace},
    };
    let Ok(tokens) = Tokenizer::new(&MySqlDialect {}, options).tokenize() else {
        return options.to_string();
    };
    let tokens = tokens
        .into_iter()
        .filter(|t| {
            !matches!(
                t,
                Token::Whitespace(Whitespace::Space | Whitespace::Newline | Whitespace::Tab)
            )
        })
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if matches!(&tokens[i],Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case("AUTO_INCREMENT"))
        {
            let value = i + 1 + usize::from(matches!(tokens.get(i + 1), Some(Token::Eq)));
            if matches!(tokens.get(value), Some(Token::Number(_, _))) {
                i = value + 1;
                continue;
            }
        }
        out.push(tokens[i].to_string());
        i += 1;
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_nested_defaults_comments_and_index_options_without_rewriting() {
        let parts = split("CREATE TABLE `t` (`a` varchar(20) DEFAULT 'a,b)\\\'', `b` int COMMENT '中文', FULLTEXT KEY `idx` (`a`) /*!50100 WITH PARSER `ngram` */) ENGINE=InnoDB").unwrap();
        assert_eq!(parts.columns.len(), 2);
        assert_eq!(index_name(&parts.constraints[0]).as_deref(), Some("idx"));
        assert!(parts.constraints[0].contains("WITH PARSER"));
        assert!(split("CREATE TABLE t(a INT); DROP TABLE t;").is_none());
        assert!(split("CREATE TABLE t(a VARCHAR(4) DEFAULT 'unterminated)").is_none());
    }
}
