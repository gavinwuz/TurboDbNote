//! Reject psql local commands while accepting SQL and streamed COPY data.
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

#[derive(Default)]
enum State {
    #[default]
    Normal,
    Single(bool),
    Double,
    Dollar(Vec<u8>),
    Comment(usize),
}
#[derive(Default)]
struct Scanner {
    state: State,
    word: Vec<u8>,
    first: Option<Vec<u8>>,
    previous: Vec<u8>,
    stdin: bool,
}
impl Scanner {
    fn word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        if self.first.is_none() {
            self.first = Some(word.clone());
        }
        if self.previous == b"from" && word == b"stdin" {
            self.stdin = true;
        }
        self.previous = word;
    }
    fn line(&mut self, line: &[u8]) -> Result<bool, String> {
        let trimmed = line.trim_ascii();
        if matches!(self.state, State::Normal)
            && self.first.is_none()
            && self.word.is_empty()
            && trimmed.starts_with(b"\\")
        {
            let safe = [b"\\restrict ".as_slice(), b"\\unrestrict ".as_slice()]
                .iter()
                .any(|prefix| {
                    trimmed.strip_prefix(*prefix).is_some_and(|key| {
                        !key.is_empty() && key.iter().all(u8::is_ascii_alphanumeric)
                    })
                });
            return if safe {
                Ok(false)
            } else {
                Err("SQL 文件含不允许的 psql 客户端命令".into())
            };
        }
        let mut i = 0;
        while i < line.len() {
            match &mut self.state {
                State::Single(escape) => {
                    if line[i] == b'\\' {
                        if !*escape {
                            return Err(
                                "SQL 文件使用了无法安全判断的普通字符串反斜杠，请使用标准 E 字符串"
                                    .into(),
                            );
                        }
                        i += 2;
                        continue;
                    }
                    if line[i] == b'\'' {
                        if line.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        self.state = State::Normal;
                    }
                    i += 1;
                }
                State::Double => {
                    if line[i] == b'"' {
                        if line.get(i + 1) == Some(&b'"') {
                            i += 2;
                            continue;
                        }
                        self.state = State::Normal;
                    }
                    i += 1;
                }
                State::Dollar(tag) => {
                    if line[i..].starts_with(tag) {
                        i += tag.len();
                        self.state = State::Normal;
                    } else {
                        i += 1;
                    }
                }
                State::Comment(depth) => {
                    if line[i..].starts_with(b"/*") {
                        *depth += 1;
                        i += 2;
                    } else if line[i..].starts_with(b"*/") {
                        *depth -= 1;
                        i += 2;
                        if *depth == 0 {
                            self.state = State::Normal;
                        }
                    } else {
                        i += 1;
                    }
                }
                State::Normal => {
                    if line[i..].starts_with(b"--") {
                        self.word();
                        break;
                    }
                    if line[i..].starts_with(b"/*") {
                        self.word();
                        self.state = State::Comment(1);
                        i += 2;
                        continue;
                    }
                    match line[i] {
                        b'\\' => {
                            return Err(
                                "SQL 文件含客户端命令，禁止执行程序、读取其他文件或切换连接".into(),
                            );
                        }
                        b'\'' => {
                            let escape = self.word == b"e";
                            self.word();
                            self.state = State::Single(escape);
                            i += 1;
                        }
                        b'"' => {
                            self.word();
                            self.state = State::Double;
                            i += 1;
                        }
                        b'$' if self.word.is_empty()
                            && !line.get(i + 1).is_some_and(u8::is_ascii_digit) =>
                        {
                            let end = (i + 1..line.len().min(i + 130))
                                .find(|j| line[*j] == b'$')
                                .ok_or("不支持的 SQL dollar-quote 标记")?;
                            let tag = &line[i + 1..end];
                            if !tag.is_empty()
                                && (!tag[0].is_ascii_alphabetic() && tag[0] != b'_'
                                    || !tag.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_'))
                            {
                                return Err("不支持的 SQL dollar-quote 标记".into());
                            }
                            self.state = State::Dollar(line[i..=end].to_vec());
                            i = end + 1;
                        }
                        b';' => {
                            self.word();
                            let copy = self.first.as_deref() == Some(b"copy") && self.stdin;
                            self.first = None;
                            self.previous.clear();
                            self.stdin = false;
                            i += 1;
                            if copy {
                                let rest = line[i..].trim_ascii();
                                if !rest.is_empty() && !rest.starts_with(b"--") {
                                    return Err("COPY FROM STDIN 语句必须独占一行".into());
                                }
                                return Ok(true);
                            }
                        }
                        b if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 128 => {
                            if self.word.len() < 256 {
                                self.word.push(b.to_ascii_lowercase());
                            }
                            i += 1;
                        }
                        _ => {
                            self.word();
                            i += 1;
                        }
                    }
                }
            }
        }
        if matches!(self.state, State::Normal) {
            self.word();
        }
        Ok(false)
    }
}
pub fn validate(path: &Path, cancelled: impl Fn() -> bool) -> Result<(), String> {
    let mut reader = BufReader::new(File::open(path).map_err(|_| "无法读取 SQL 文件")?);
    let mut scanner = Scanner::default();
    let mut copy = false;
    let mut first = true;
    loop {
        if cancelled() {
            return Err("任务已取消".into());
        }
        if copy {
            let mut prefix = Vec::new();
            let mut length = 0;
            loop {
                if cancelled() {
                    return Err("任务已取消".into());
                }
                let mut part = Vec::new();
                let n = reader
                    .by_ref()
                    .take(65536)
                    .read_until(b'\n', &mut part)
                    .map_err(|_| "无法读取 COPY 数据")?;
                if prefix.len() < 4 {
                    prefix.extend(part.iter().take(4 - prefix.len()));
                }
                length += n;
                if n == 0 || part.last() == Some(&b'\n') {
                    if length <= 4
                        && [
                            b"\\.\n".as_slice(),
                            b"\\.\r\n".as_slice(),
                            b"\\.".as_slice(),
                        ]
                        .contains(&prefix.as_slice())
                    {
                        copy = false;
                        break;
                    }
                    if n == 0 {
                        return Err("COPY 数据缺少结束标记".into());
                    }
                    break;
                }
            }
            continue;
        }
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "无法读取 SQL 文件")?;
        if n == 0 {
            break;
        }
        if n > 8 * 1024 * 1024 {
            return Err("单行 SQL 超过 8 MiB；大数据请使用 COPY 格式".into());
        }
        let bytes = if first {
            first = false;
            line.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&line)
        } else {
            &line
        };
        copy = scanner.line(bytes)?;
    }
    if copy {
        return Err("COPY 数据缺少结束标记".into());
    }
    if !matches!(scanner.state, State::Normal) {
        return Err("SQL 文件含未闭合的字符串或注释".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn check(sql: &str) -> bool {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), sql).unwrap();
        validate(file.path(), || false).is_ok()
    }
    #[test]
    fn sql_and_copy_are_allowed_but_client_commands_are_not() {
        assert!(check(
            "CREATE TABLE t(v text);\nCOPY t FROM stdin;\n\\N\n\\! is just data\n\\.\n"
        ));
        assert!(check(
            "DO $body$ BEGIN RAISE NOTICE '\\!'; END $body$;\nSELECT E'a\\\\b';"
        ));
        assert!(check("\\restrict abc123\nSELECT 1;\n\\unrestrict abc123\n"));
        for sql in [
            "\\! program\n",
            "SELECT 1; \\! program\n",
            "COPY t FROM stdin; \\! program\n",
            "COPY t FROM stdin;\nx\n\\.\n\\i file\n",
            "\\restrict abc123 \\! program\n",
            "SELECT 'a\\'; \\! program\n",
        ] {
            assert!(!check(sql));
        }
        assert!(!check(&format!(
            "\\restrict {} \\! program\n",
            "a".repeat(4096)
        )));
    }
}
