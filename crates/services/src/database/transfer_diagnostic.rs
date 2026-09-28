//! Bounded diagnostics: raw stderr is discarded, only fixed categories and known codes survive.
use std::io::Read;

#[derive(Default)]
pub(super) struct Diagnostic {
    code: Option<u32>,
    hint: Option<&'static str>,
    option: Option<&'static str>,
}
impl Diagnostic {
    pub(super) fn read(mut stream: impl Read) -> Self {
        let mut prefix = Vec::new();
        let _ = stream.by_ref().take(16 * 1024).read_to_end(&mut prefix);
        let _ = std::io::copy(&mut stream, &mut std::io::sink());
        Self::parse(&prefix)
    }
    fn parse(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes).to_lowercase();
        let known = [
            1007, 1044, 1045, 1050, 1054, 1062, 1064, 1142, 1146, 1193, 1227, 1267, 1698, 2002,
            2003, 2005, 2006, 2013, 2026, 2055,
        ];
        let code = text.lines().find_map(|line| {
            let number = line
                .trim_start()
                .strip_prefix("error ")
                .or_else(|| line.trim_start().strip_prefix("mysqldump: got error: "))?;
            let digits = number
                .chars()
                .take_while(char::is_ascii_digit)
                .take(5)
                .collect::<String>();
            digits
                .parse::<u32>()
                .ok()
                .filter(|code| known.contains(code))
        });
        let option = [
            "secure-auth",
            "ssl-mode",
            "binary-mode",
            "column-statistics",
            "set-gtid-purged",
            "no-tablespaces",
            "rows-per-insert",
            "single-transaction",
        ]
        .into_iter()
        .find(|name| text.contains(name));
        let hint = if text.contains("unknown option")
            || text.contains("unrecognized option")
            || text.contains("unknown variable")
        {
            "客户端参数不兼容，请检查 mysql、mysqldump 等工具的具体版本与支持选项"
        } else if matches!(code, Some(1045 | 1698))
            || text.contains("password authentication failed")
        {
            "客户端认证失败，请重新连接并核对该目标的用户名、密码及认证方式"
        } else if matches!(code, Some(1007 | 1050 | 1062))
            || text.contains("already exists")
            || text.contains("duplicate key")
        {
            "目标库、表或主键已存在，请选择新目标，或核对已有库的追加/更新模式"
        } else if matches!(code, Some(1044 | 1142 | 1227))
            || text.contains("permission denied")
            || text.contains("access denied")
        {
            "客户端权限不足，请检查目标库及相应操作权限"
        } else if matches!(code, Some(1054 | 1064 | 1146 | 1193 | 1267))
            || text.contains("syntax error")
            || text.contains("unknown system variable")
        {
            "SQL 或元数据与服务器版本、表结构不兼容，请核对源和目标版本"
        } else if matches!(code, Some(2026))
            || text.contains("ssl")
            || text.contains("tls")
            || text.contains("certificate")
        {
            "TLS 连接失败，请核对证书和已保存的 TLS 策略；不会自动降低保护"
        } else if matches!(code, Some(2002 | 2003 | 2005 | 2006 | 2013 | 2055))
            || text.contains("connection refused")
            || text.contains("could not connect")
        {
            "网络连接失败或中断，请检查目标地址、端口与服务状态"
        } else if text.contains("no space left")
            || text.contains("disk full")
            || text.contains("error on write")
        {
            "文件写入失败，请检查磁盘空间和输出目录权限"
        } else {
            return Self {
                code,
                hint: None,
                option: None,
            };
        };
        Self {
            code,
            hint: Some(hint),
            option: if text.contains("unknown option") || text.contains("unrecognized option") {
                option
            } else {
                None
            },
        }
    }
    pub(super) fn detail(&self) -> Option<String> {
        self.hint.map(|hint| {
            let mut text = match self.code {
                Some(code) => format!("{hint}（错误码 {code}）"),
                None => hint.into(),
            };
            if let Some(option) = self.option {
                text.push_str(&format!("；不支持选项：--{option}"));
            }
            text
        })
    }
    pub(super) fn failure(&self, exit: i32, writing: bool) -> String {
        let detail = self.detail().unwrap_or_else(|| {
            "客户端未提供可安全展示的错误类别，请查看输出中的失败阶段并核对客户端版本".into()
        });
        format!(
            "{detail}；退出码 {exit}。{}",
            if writing {
                "目标可能保留已写入内容，请核对后再操作；不会自动重试"
            } else {
                "导出未完成，不会自动重试"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exit_two_is_classified_without_echoing_remote_data() {
        let sensitive = uuid::Uuid::new_v4().to_string();
        let raw = format!("mysqldump: unknown option '--skip-secure-auth' {sensitive}");
        let failure = Diagnostic::parse(raw.as_bytes()).failure(2, false);
        assert!(
            failure.contains("参数不兼容")
                && !failure.contains(&sensitive)
                && !failure.contains("已写入")
        );
        let raw = format!("ERROR 1045 (28000): Access denied {sensitive}");
        let failure = Diagnostic::parse(raw.as_bytes()).failure(1, true);
        assert!(
            failure.contains("认证失败")
                && failure.contains("1045")
                && !failure.contains(&sensitive)
        );
        let raw = format!("ERROR 1062 (23000): Duplicate entry {sensitive}");
        assert!(
            !Diagnostic::parse(raw.as_bytes())
                .failure(1, true)
                .contains(&sensitive)
        );
    }
    #[test]
    fn diagnostics_only_inspect_bounded_prefix_but_drain_the_stream() {
        let mut bytes = vec![b'x'; 32768];
        bytes.extend_from_slice(b"unknown option");
        let mut input = std::io::Cursor::new(bytes);
        let diagnostic = Diagnostic::read(&mut input);
        assert!(diagnostic.hint.is_none() && input.position() == input.get_ref().len() as u64);
    }
}
