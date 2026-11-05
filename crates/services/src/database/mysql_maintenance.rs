//! Bounded MySQL maintenance grammar, shared by Note and SQL-file execution.
//! Unknown commands are never accepted through an arbitrary prefix fallback.
use sqlparser::{
    dialect::MySqlDialect,
    tokenizer::{Token, Tokenizer},
};
#[derive(Clone, Debug)]
pub(crate) struct Command {
    pub read: bool,
    pub tables: Vec<Vec<String>>,
}
fn word(t: &Token) -> Option<String> {
    match t {
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
        _ => None,
    }
}
pub(crate) fn classify(sql: &str) -> Option<Command> {
    if sql.len() > 60_000
        || sql.contains(['\0', '\\'])
        || sql.contains("/*!")
        || sql.contains("/*M!")
    {
        return None;
    }
    let mut ts = Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize()
        .ok()?
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    if ts.last() == Some(&Token::SemiColon) {
        ts.pop();
    }
    if ts.is_empty() || ts.contains(&Token::SemiColon) {
        return None;
    }
    let words = ts
        .iter()
        .map(|t| word(t).unwrap_or_default())
        .collect::<Vec<_>>();
    let text = words.join(" ");
    let read = matches!(
        text.as_str(),
        "SHOW MASTER STATUS"
            | "SHOW BINARY LOG STATUS"
            | "SHOW BINARY LOGS"
            | "SHOW MASTER LOGS"
            | "SHOW SLAVE STATUS"
            | "SHOW REPLICA STATUS"
            | "SHOW SLAVE HOSTS"
            | "SHOW REPLICAS"
            | "SHOW PROCESSLIST"
            | "SHOW FULL PROCESSLIST"
            | "SHOW ENGINE INNODB STATUS"
            | "SHOW ENGINES"
            | "SHOW STORAGE ENGINES"
            | "SHOW PLUGINS"
            | "SHOW PRIVILEGES"
            | "SHOW WARNINGS"
            | "SHOW ERRORS"
            | "SHOW PROFILES"
            | "SHOW OPEN TABLES"
            | "SHOW TABLE STATUS"
    );
    if read {
        return Some(Command {
            read: true,
            tables: vec![],
        });
    }
    if matches!(
        text.as_str(),
        "FLUSH STATUS"
            | "FLUSH LOGS"
            | "FLUSH BINARY LOGS"
            | "FLUSH ENGINE LOGS"
            | "FLUSH ERROR LOGS"
            | "FLUSH GENERAL LOGS"
            | "FLUSH SLOW LOGS"
            | "FLUSH RELAY LOGS"
            | "FLUSH HOSTS"
            | "FLUSH PRIVILEGES"
            | "FLUSH TABLES"
            | "START REPLICA"
            | "STOP REPLICA"
            | "START SLAVE"
            | "STOP SLAVE"
            | "START REPLICA IO_THREAD"
            | "STOP REPLICA IO_THREAD"
            | "START REPLICA SQL_THREAD"
            | "STOP REPLICA SQL_THREAD"
    ) {
        return Some(Command {
            read: false,
            tables: vec![],
        });
    }
    if words[0] == "KILL" {
        let n = if matches!(
            words.get(1).map(String::as_str),
            Some("QUERY" | "CONNECTION")
        ) {
            2
        } else {
            1
        };
        if ts.len() == n + 1 && matches!(&ts[n],Token::Number(v,_) if v.parse::<u64>().is_ok()) {
            return Some(Command {
                read: false,
                tables: vec![],
            });
        }
    }
    // Only one supported global variable with a literal value: no expressions,
    // credential changes, file destinations, PERSIST or multi-assignment.
    if words.first().map(String::as_str) == Some("SET")
        && words.get(1).map(String::as_str) == Some("GLOBAL")
        && ts.len() == 5
        && ts.get(3) == Some(&Token::Eq)
        && matches!(
            words[2].as_str(),
            "BINLOG_FORMAT"
                | "BINLOG_ROW_IMAGE"
                | "BINLOG_EXPIRE_LOGS_SECONDS"
                | "EXPIRE_LOGS_DAYS"
                | "SLOW_QUERY_LOG"
                | "LONG_QUERY_TIME"
                | "GENERAL_LOG"
                | "MAX_CONNECTIONS"
        )
        && matches!(
            &ts[4],
            Token::SingleQuotedString(_) | Token::Number(_, _) | Token::Word(_)
        )
    {
        return Some(Command {
            read: false,
            tables: vec![],
        });
    }
    // Table maintenance returns diagnostic rowsets and requires frozen table guards.
    if !matches!(
        words[0].as_str(),
        "ANALYZE" | "CHECK" | "OPTIMIZE" | "REPAIR"
    ) {
        return None;
    }
    let mut i = 1;
    if matches!(
        words.get(i).map(String::as_str),
        Some("LOCAL" | "NO_WRITE_TO_BINLOG")
    ) {
        i += 1;
    }
    if words.get(i).map(String::as_str) != Some("TABLE") {
        return None;
    }
    i += 1;
    let mut tables = vec![];
    loop {
        let Token::Word(w) = ts.get(i)? else {
            return None;
        };
        let mut parts = vec![w.value.clone()];
        i += 1;
        if ts.get(i) == Some(&Token::Period) {
            i += 1;
            let Token::Word(w) = ts.get(i)? else {
                return None;
            };
            parts.push(w.value.clone());
            i += 1;
        }
        tables.push(parts);
        if tables.len() > 16 {
            return None;
        }
        if ts.get(i) == Some(&Token::Comma) {
            i += 1;
        } else {
            break;
        }
    }
    if words[i..].iter().any(|w| {
        !matches!(
            w.as_str(),
            "QUICK" | "FAST" | "MEDIUM" | "EXTENDED" | "CHANGED" | "FOR" | "UPGRADE" | "USE_FRM"
        )
    }) {
        return None;
    }
    Some(Command {
        read: false,
        tables,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_categories_do_not_accept_arbitrary_sql() {
        for sql in [
            "SHOW MASTER STATUS",
            "SHOW BINARY LOG STATUS; # tail",
            "SHOW FULL PROCESSLIST",
            "SHOW ENGINE INNODB STATUS",
        ] {
            assert!(classify(sql).unwrap().read);
        }
        for sql in [
            "FLUSH LOGS",
            "KILL QUERY 42",
            "STOP REPLICA",
            "ANALYZE TABLE `db`.`a`, b",
        ] {
            assert!(!classify(sql).unwrap().read);
        }
        for sql in [
            "SHOW MASTER STATUS; DELETE FROM x",
            "FLUSH TABLES WITH READ LOCK",
            "RESET MASTER",
            "PURGE BINARY LOGS TO 'x'",
            "KILL evil()",
            "ANALYZE TABLE a; DROP TABLE b",
            "SHOW ENGINE evil() STATUS",
        ] {
            assert!(classify(sql).is_none(), "{sql}");
        }
        assert_eq!(
            classify("CHECK TABLE db.a").unwrap().tables,
            vec![vec!["db".to_string(), "a".to_string()]]
        );
    }
}
