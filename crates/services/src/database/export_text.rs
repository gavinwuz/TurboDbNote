use std::io::{Read, Write};

/// Streaming normalization of statement lines. Quoted payloads are preserved byte for
/// byte: changing a newline inside a SQL literal can change restored data.
pub(super) fn write_sql(
    input: impl Read,
    mut out: impl Write,
    crlf: bool,
    kind: crate::DatabaseKind,
    mut check: impl FnMut() -> std::io::Result<()>,
) -> std::io::Result<()> {
    out.write_all(if crlf {
        b"-- Exported by TurboDBNote\r\n"
    } else {
        b"-- Exported by TurboDBNote\n"
    })?;
    out.write_all(if crlf {
        b"-- Existing tables are kept without schema comparison; data statements still execute.\r\n"
    } else {
        b"-- Existing tables are kept without schema comparison; data statements still execute.\n"
    })?;
    let mut create_prefix = 0u8; // 0: statement start, 1: CREATE, 2: TABLE, 3: other.
    let mut input = std::io::BufReader::new(input).bytes().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut backslash = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut dollar = Vec::<u8>::new();
    let mut dollar_match = 0usize;
    let mut previous = 0u8;
    let mut count = 0usize;
    while let Some(byte) = input.next() {
        let byte = byte?;
        count += 1;
        if count.is_multiple_of(65536) {
            check()?;
        }
        if !dollar.is_empty() {
            out.write_all(&[byte])?;
            if byte == dollar[dollar_match] {
                dollar_match += 1;
                if dollar_match == dollar.len() {
                    dollar.clear();
                    dollar_match = 0;
                }
            } else {
                dollar_match = usize::from(byte == b'$');
            }
        } else if let Some(q) = quote {
            out.write_all(&[byte])?;
            if escaped {
                escaped = false;
            } else if backslash && byte == b'\\' {
                escaped = true;
            } else if byte == q {
                if input
                    .peek()
                    .is_some_and(|b| b.as_ref().is_ok_and(|b| *b == q))
                {
                    out.write_all(&[input.next().unwrap()?])?;
                } else {
                    quote = None;
                }
            }
        } else if byte == b'\r' || byte == b'\n' {
            if byte == b'\r'
                && input
                    .peek()
                    .is_some_and(|b| b.as_ref().is_ok_and(|b| *b == b'\n'))
            {
                input.next();
            }
            out.write_all(if crlf { b"\r\n" } else { b"\n" })?;
            line_comment = false;
        } else if line_comment {
            out.write_all(&[byte])?;
        } else if block_comment {
            out.write_all(&[byte])?;
            if previous == b'*' && byte == b'/' {
                block_comment = false;
            }
        } else if (byte == b'-'
            && input
                .peek()
                .is_some_and(|b| b.as_ref().is_ok_and(|b| *b == b'-')))
            || (byte == b'#' && kind == crate::DatabaseKind::MySql)
        {
            line_comment = true;
            out.write_all(&[byte])?;
        } else if byte == b'/'
            && input
                .peek()
                .is_some_and(|b| b.as_ref().is_ok_and(|b| *b == b'*'))
        {
            block_comment = true;
            out.write_all(&[byte])?;
        } else if byte == b'$' && kind == crate::DatabaseKind::PostgreSql {
            let mut tag = vec![byte];
            while tag.len() < 128
                && input.peek().is_some_and(|b| {
                    b.as_ref()
                        .is_ok_and(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'$'))
                })
            {
                let b = input.next().unwrap()?;
                tag.push(b);
                if b == b'$' {
                    dollar = tag.clone();
                    break;
                }
            }
            out.write_all(&tag)?;
        } else if create_prefix != 3 && (byte.is_ascii_alphanumeric() || byte == b'_') {
            let mut token = vec![byte];
            while token.len() < 64
                && input.peek().is_some_and(|b| {
                    b.as_ref()
                        .is_ok_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                })
            {
                token.push(input.next().unwrap()?);
            }
            if create_prefix == 2 && !token.eq_ignore_ascii_case(b"IF") {
                out.write_all(b"IF NOT EXISTS ")?;
            }
            create_prefix = match create_prefix {
                0 if token.eq_ignore_ascii_case(b"CREATE") => 1,
                1 if token.eq_ignore_ascii_case(b"TABLE") => 2,
                1 if kind == crate::DatabaseKind::PostgreSql
                    && token.eq_ignore_ascii_case(b"UNLOGGED") =>
                {
                    1
                }
                _ => 3,
            };
            out.write_all(&token)?;
            previous = *token.last().unwrap();
            continue;
        } else {
            if create_prefix == 2
                && (matches!(byte, b'"' | b'`' | b'[')
                    || byte == b'\'' && kind == crate::DatabaseKind::Sqlite
                    || byte >= 128)
            {
                out.write_all(b"IF NOT EXISTS ")?;
                create_prefix = 3;
            }
            if byte == b';' {
                create_prefix = 0;
            } else if !byte.is_ascii_whitespace() {
                create_prefix = 3;
            }
            if matches!(byte, b'\'' | b'"' | b'`')
                || byte == b'[' && kind == crate::DatabaseKind::Sqlite
            {
                quote = Some(if byte == b'[' { b']' } else { byte });
                backslash = byte == b'\''
                    && (kind == crate::DatabaseKind::MySql
                        || (kind == crate::DatabaseKind::PostgreSql
                            && matches!(previous, b'E' | b'e')));
            }
            out.write_all(&[byte])?;
        }
        previous = byte;
    }
    check()?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_guards_preserve_original_definitions_and_payloads() {
        for kind in [
            crate::DatabaseKind::MySql,
            crate::DatabaseKind::Sqlite,
            crate::DatabaseKind::PostgreSql,
        ] {
            let sql = b"-- CREATE TABLE comment(x);\n/* ignored */ CREATE /* note */ TABLE `odd;name`(v TEXT DEFAULT 'CREATE TABLE data');\nCREATE TABLE IF NOT EXISTS kept(id INT);\nINSERT INTO kept VALUES('CREATE TABLE literal');";
            let mut once = Vec::new();
            write_sql(&sql[..], &mut once, false, kind, || Ok(())).unwrap();
            let text = String::from_utf8(once.clone()).unwrap();
            assert!(text.contains("CREATE /* note */ TABLE IF NOT EXISTS `odd;name`"));
            assert!(text.contains("DEFAULT 'CREATE TABLE data'"));
            assert!(text.contains("CREATE TABLE IF NOT EXISTS kept"));
            assert!(!text.contains("IF NOT EXISTS IF NOT EXISTS"));
            let mut twice = Vec::new();
            write_sql(&once[..], &mut twice, false, kind, || Ok(())).unwrap();
            assert!(
                !String::from_utf8(twice)
                    .unwrap()
                    .contains("IF NOT EXISTS IF NOT EXISTS")
            );
        }
    }
    #[test]
    fn repeated_sqlite_structure_export_keeps_existing_table_and_data() {
        let mut out = Vec::new();
        write_sql(
            &b"CREATE TABLE [a;b](id INTEGER PRIMARY KEY); CREATE TABLE 'single'(v TEXT);"[..],
            &mut out,
            false,
            crate::DatabaseKind::Sqlite,
            || Ok(()),
        )
        .unwrap();
        let sql = String::from_utf8(out).unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS 'single'"));
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(&sql).unwrap();
        db.execute_batch("INSERT INTO [a;b] VALUES(7);").unwrap();
        db.execute_batch(&sql).unwrap();
        assert_eq!(
            db.query_row("SELECT id FROM [a;b]", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
    }
    #[test]
    fn view_placeholders_and_routine_bodies_are_not_rewritten() {
        let sql = b"CREATE TEMPORARY TABLE placeholder(id INT);\nCREATE VIEW v AS SELECT 'CREATE TABLE t';\nCREATE FUNCTION f() RETURNS void AS $body$BEGIN CREATE TABLE hidden(id INT); END;$body$ LANGUAGE plpgsql;\nCREATE UNLOGGED TABLE real(id INT);";
        let mut out = Vec::new();
        write_sql(
            &sql[..],
            &mut out,
            false,
            crate::DatabaseKind::PostgreSql,
            || Ok(()),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("CREATE TEMPORARY TABLE placeholder"));
        assert!(text.contains("BEGIN CREATE TABLE hidden"));
        assert!(text.contains("CREATE UNLOGGED TABLE IF NOT EXISTS real"));
    }
    #[test]
    fn comments_and_postgres_dollar_strings_are_preserved() {
        let sql =
            b"-- writer's comment\r\nSELECT $body$a\r\nb$body$;\r\n/* 'comment */\r\nSELECT 2;";
        let mut out = Vec::new();
        write_sql(
            &sql[..],
            &mut out,
            false,
            crate::DatabaseKind::PostgreSql,
            || Ok(()),
        )
        .unwrap();
        let value = String::from_utf8(out).unwrap();
        assert!(value.contains("comment\nSELECT"));
        assert!(value.contains("$body$a\r\nb$body$;\n"));
        assert!(value.contains("/* 'comment */\nSELECT"));
    }
    #[test]
    fn line_endings_and_header_preserve_literal_data() {
        let sql = b"SELECT 'a\r\nb';\r\nSELECT 2;\n";
        for crlf in [false, true] {
            let mut out = Vec::new();
            write_sql(
                &sql[..],
                &mut out,
                crlf,
                crate::DatabaseKind::Sqlite,
                || Ok(()),
            )
            .unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(text.starts_with("-- Exported by TurboDBNote"));
            assert!(text.contains("'a\r\nb'"));
            assert!(text.contains(if crlf { ";\r\nSELECT" } else { ";\nSELECT" }));
        }
    }
}
