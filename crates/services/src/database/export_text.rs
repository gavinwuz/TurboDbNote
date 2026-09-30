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
        } else {
            if matches!(byte, b'\'' | b'"' | b'`') {
                quote = Some(byte);
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
