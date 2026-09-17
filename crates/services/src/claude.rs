//! Claude Code CLI print-mode adapter. Every turn owns a process; session IDs
//! provide continuity. No shell wrappers, bypass flags, or Codex RPC assumptions.
use crate::codex::{ClientCommand, Connection, Event, Model, Prompt};
use async_channel::{Receiver, Sender};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};
const LIMIT: usize = 1024 * 1024;

fn command(path: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.current_dir(cwd).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}
async fn inspect(path: &Path, cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = command(path, cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut out = child
        .stdout
        .take()
        .ok_or("Claude stdout 不可用")?
        .take(65537);
    let result = tokio::time::timeout(Duration::from_secs(12), async {
        let mut bytes = vec![];
        out.read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("Claude 状态输出过大".into());
        }
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        if !status.success() && args != ["auth", "status", "--json"] {
            return Err("Claude CLI 检查失败，请确认已安装兼容版本".into());
        }
        Ok(text)
    })
    .await
    .map_err(|_| "Claude 状态检查超时".to_string());
    let _ = child.kill().await;
    let _ = child.wait().await;
    result?
}
pub fn models() -> Vec<Model> {
    [
        ("default", "Claude 默认（CLI 配置）"),
        ("sonnet", "Sonnet（别名）"),
        ("opus", "Opus（别名）"),
        ("haiku", "Haiku（别名）"),
    ]
    .into_iter()
    .map(|(id, label)| Model {
        id: id.into(),
        model: id.into(),
        display_name: label.into(),
        default_reasoning_effort: String::new(),
        supported_reasoning_efforts: vec![],
        is_default: id == "default",
    })
    .collect()
}
pub fn start(path: PathBuf, cwd: PathBuf) -> (Connection, Receiver<Event>) {
    let (commands, rx) = async_channel::bounded(16);
    let (events, output) = async_channel::bounded(128);
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(async {
                if let Err(error) = run(path, cwd, rx, events.clone()).await {
                    let _ = events.send(Event::Error(error)).await;
                }
                let _ = events.send(Event::Disconnected).await;
            }),
            Err(error) => {
                let _ = events.send_blocking(Event::Error(error.to_string()));
            }
        }
    });
    (Connection::from_sender(commands), output)
}
async fn run(
    path: PathBuf,
    cwd: PathBuf,
    commands: Receiver<ClientCommand>,
    events: Sender<Event>,
) -> Result<(), String> {
    if !path.is_absolute() || !path.is_file() || !cwd.is_absolute() || !cwd.is_dir() {
        return Err("请选择有效的 Claude 可执行文件和工作目录绝对路径".into());
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("请选择实际 claude.exe，不支持 .cmd / .ps1".into());
    }
    let version = inspect(&path, &cwd, &["--version"]).await?;
    if !version.contains("Claude Code") {
        return Err("该文件不是 Claude Code CLI".into());
    }
    let help = inspect(&path, &cwd, &["--help"]).await?;
    for flag in [
        "--include-partial-messages",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--tools",
        "--setting-sources",
    ] {
        if !help.contains(flag) {
            return Err(format!("Claude CLI 缺少 {flag}，请升级后重试"));
        }
    }
    let auth: Value =
        serde_json::from_str(&inspect(&path, &cwd, &["auth", "status", "--json"]).await?)
            .map_err(|_| "无法解析 Claude 登录状态")?;
    let ready = auth["loggedIn"].as_bool().unwrap_or(false);
    events
        .send(Event::Log(format!("Claude Code {}", version.trim())))
        .await
        .map_err(|e| e.to_string())?;
    events
        .send(Event::Account {
            ready,
            label: if ready {
                "Claude 登录状态可用"
            } else {
                "请先使用 Claude Code CLI 登录，再重新连接"
            }
            .into(),
        })
        .await
        .map_err(|e| e.to_string())?;
    events
        .send(Event::Models(models()))
        .await
        .map_err(|e| e.to_string())?;
    let mut session: Option<String> = None;
    while let Ok(cmd) = commands.recv().await {
        match cmd {
            ClientCommand::NewThread => session = None,
            ClientCommand::RestoreThread(id) => {
                if uuid::Uuid::parse_str(&id).is_err() {
                    return Err("Claude 历史会话 ID 无效，请新建会话".into());
                }
                session = Some(id);
            }
            ClientCommand::Interrupt => {}
            ClientCommand::Send(prompt) => {
                if !ready {
                    events
                        .send(Event::Error("Claude 尚未登录".into()))
                        .await
                        .map_err(|e| e.to_string())?;
                    continue;
                }
                crate::codex::validate_model(&models(), &prompt)?;
                if prompt.text.trim().is_empty() || prompt.text.len() > 65536 {
                    return Err("Claude 输入必须为 1–65536 字节".into());
                }
                if !turn(&path, &cwd, prompt, &mut session, &commands, &events).await? {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}
fn turn_args(model: &str, session: Option<&str>) -> Vec<String> {
    let mut args=vec!["--print","--output-format","stream-json","--verbose","--include-partial-messages",
        "--input-format","text","--tools","","--permission-mode","dontAsk","--strict-mcp-config","--mcp-config",r#"{"mcpServers":{}}"#,
        "--disable-slash-commands","--setting-sources","user","--settings",r#"{"disableAllHooks":true,"enabledPlugins":{}}"#,
        "--no-chrome","--system-prompt","You are TurboDbNote's SQL analysis assistant. Explain SQL and propose text only. No tools or database access are available. Do not request credentials."]
        .into_iter().map(String::from).collect::<Vec<_>>();
    if model != "default" {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(id) = session {
        args.extend(["--resume".into(), id.into()]);
    }
    args
}
async fn read_frames(reader: impl AsyncRead + Unpin, tx: Sender<Result<Value, String>>) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut data = vec![];
        let result = (&mut reader)
            .take((LIMIT + 1) as u64)
            .read_until(b'\n', &mut data)
            .await;
        let value = match result {
            Ok(0) => break,
            Ok(_) if data.len() > LIMIT => Err("Claude 输出帧超过 1 MiB".into()),
            Ok(_) => serde_json::from_slice(&data).map_err(|_| "Claude 返回无效的流式 JSON".into()),
            Err(e) => Err(e.to_string()),
        };
        let error = value.is_err();
        if tx.send(value).await.is_err() || error {
            break;
        }
    }
}
#[derive(Default)]
struct Stream {
    text: String,
    done: bool,
    failed: bool,
    session: Option<String>,
}
impl Stream {
    fn parse(&mut self, value: &Value, item: &str) -> Result<Vec<Event>, String> {
        let mut events = vec![];
        if let Some(session) = value["session_id"].as_str()
            && self.session.as_deref() != Some(session)
        {
            if uuid::Uuid::parse_str(session).is_err() {
                return Err("Claude 返回无效会话 ID".into());
            }
            self.session = Some(session.into());
            events.push(Event::Thread(session.into()));
        }
        match value["type"].as_str() {
            Some("stream_event") if value["event"]["delta"]["type"] == "text_delta" => {
                if let Some(text) = value["event"]["delta"]["text"].as_str() {
                    if self.text.len() + text.len() > LIMIT {
                        return Err("Claude 回复超过显示限制".into());
                    }
                    self.text.push_str(text);
                    events.push(Event::Delta {
                        item: item.into(),
                        text: text.into(),
                    });
                }
            }
            Some("assistant") => {
                // Complete assistant events replace deltas; never append a second copy.
                if let Some(parts) = value["message"]["content"].as_array() {
                    let text = parts
                        .iter()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        if text.len() > LIMIT {
                            return Err("Claude 回复超过显示限制".into());
                        }
                        self.text = text.clone();
                        events.push(Event::Snapshot {
                            item: item.into(),
                            text,
                        });
                    }
                }
            }
            Some("result") => {
                self.done = true;
                self.failed =
                    value["is_error"] == true || value["subtype"].as_str() != Some("success");
                if self.failed {
                    let error = value["errors"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .filter(|s| !s.is_empty())
                        .or_else(|| value["result"].as_str().map(String::from))
                        .unwrap_or("Claude 请求失败".into());
                    events.push(Event::Error(error.chars().take(2000).collect()));
                } else if let Some(text) = value["result"].as_str()
                    && !text.is_empty()
                {
                    if text.len() > LIMIT {
                        return Err("Claude 回复超过显示限制".into());
                    }
                    self.text = text.into();
                    events.push(Event::Snapshot {
                        item: item.into(),
                        text: text.into(),
                    });
                }
            }
            Some("system") if value["subtype"] == "api_retry" => {
                events.push(Event::Log("Claude 正在重试模型请求".into()))
            }
            _ => {}
        }
        Ok(events)
    }
}
async fn turn(
    path: &Path,
    cwd: &Path,
    prompt: Prompt,
    session: &mut Option<String>,
    commands: &Receiver<ClientCommand>,
    events: &Sender<Event>,
) -> Result<bool, String> {
    let mut child = command(path, cwd)
        .args(turn_args(&prompt.model, session.as_deref()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 Claude 失败：{e}"))?;
    #[cfg(windows)]
    let _tree = crate::codex::job::ProcessTree::attach(&child)?;
    let (tx, rx) = async_channel::bounded(128);
    let out = child.stdout.take().ok_or("Claude stdout 不可用")?;
    let mut err = child.stderr.take().ok_or("Claude stderr 不可用")?;
    let reader = tokio::spawn(read_frames(out, tx));
    let diagnostics = tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        while err.read(&mut buffer).await.unwrap_or(0) > 0 {}
    });
    let result = run_turn(&mut child, prompt, session, commands, events, rx).await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    reader.abort();
    diagnostics.abort();
    result
}
async fn run_turn(
    child: &mut Child,
    prompt: Prompt,
    session: &mut Option<String>,
    commands: &Receiver<ClientCommand>,
    events: &Sender<Event>,
    rx: Receiver<Result<Value, String>>,
) -> Result<bool, String> {
    let mut input = child.stdin.take().ok_or("Claude stdin 不可用")?;
    tokio::time::timeout(Duration::from_secs(5), async {
        input.write_all(prompt.text.as_bytes()).await?;
        input.shutdown().await
    })
    .await
    .map_err(|_| "Claude 输入超时")?
    .map_err(|e| e.to_string())?;
    drop(input);
    events
        .send(Event::Log("Claude 开始文本分析（工具已禁用）".into()))
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = Stream::default();
    let mut failure = None;
    let item = format!("claude-{}", uuid::Uuid::new_v4());
    let timeout = tokio::time::sleep(Duration::from_secs(300));
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            cmd=commands.recv()=>match cmd{
                Err(_)=>return Ok(false),
                Ok(ClientCommand::Interrupt)=>{
                    // Windows print-mode has no reliable console SIGINT. Terminate
                    // this turn, keep the partial reply, and reap the entire job tree.
                    let _=child.kill().await;let _=child.wait().await;
                    events.send(Event::Completed("interrupted".into())).await.map_err(|e|e.to_string())?;
                    return Ok(true);
                }
                Ok(_)=>{events.send(Event::Log("请等待当前 Claude 请求结束".into())).await.map_err(|e|e.to_string())?;}
            },
            frame=rx.recv()=>match frame{
                Ok(value)=>{
                    for event in stream.parse(&value?,&item)?{
                        if let Event::Error(error)=event{failure=Some(error);}else{events.send(event).await.map_err(|e|e.to_string())?;}
                    }
                    if stream.session.is_some(){*session=stream.session.clone();}
                }
                Err(_)=>{
                    let status=tokio::time::timeout(Duration::from_secs(5),child.wait()).await.map_err(|_|"Claude 退出超时")?.map_err(|e|e.to_string())?;
                    if !stream.done{return Err(format!("Claude 未返回完成事件便退出（{}），请检查 CLI 登录/配置",status));}
                    if !status.success()&&!stream.failed{return Err("Claude 进程异常退出".into());}
                    events.send(Event::Completed(if stream.failed{"failed"}else{"completed"}.into())).await.map_err(|e|e.to_string())?;
                    if let Some(error)=failure{events.send(Event::Error(error)).await.map_err(|e|e.to_string())?;}
                    return Ok(true);
                }
            },
            _=&mut timeout=>return Err("Claude 请求超过 5 分钟，已终止".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn stream_replaces_full_text_and_captures_session() {
        let mut s = Stream::default();
        let id = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            s.parse(&json!({"type":"system","session_id":id}), "i")
                .unwrap()[0],
            Event::Thread(_)
        ));
        assert!(matches!(
            s.parse(
                &json!({"type":"stream_event","event":{"delta":{"type":"text_delta","text":"Hi"}}}),
                "i"
            )
            .unwrap()[0],
            Event::Delta { .. }
        ));
        let events=s.parse(&json!({"type":"assistant","message":{"content":[{"type":"text","text":"Hi there"}]}}),"i").unwrap();
        assert!(matches!(&events[0],Event::Snapshot{text,..} if text=="Hi there"));
        assert_eq!(s.text, "Hi there");
    }
    #[test]
    fn error_result_is_not_success() {
        let mut s = Stream::default();
        let events=s.parse(&json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["bad model"]}),"i").unwrap();
        assert!(s.done && s.failed);
        assert!(matches!(&events[0],Event::Error(e) if e=="bad model"));
    }
    #[test]
    fn args_do_not_enable_tools_or_inherit_codex_effort() {
        let args = turn_args("sonnet", Some("session"));
        assert!(args.windows(2).any(|a| a == ["--tools", ""]));
        assert!(args.windows(2).any(|a| a == ["--resume", "session"]));
        assert!(!args.iter().any(|a| a == "--effort" || a.contains("bypass")));
        assert!(!turn_args("default", None).iter().any(|a| a == "--model"));
    }
    #[tokio::test]
    async fn malformed_frame_fails() {
        let (tx, rx) = async_channel::bounded(4);
        read_frames(&b"bad\n"[..], tx).await;
        assert!(rx.recv().await.unwrap().is_err());
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn interrupt_terminates_only_current_turn() {
        let mut child = command(
            Path::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"),
            &std::env::temp_dir(),
        )
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::In.ReadToEnd() | Out-Null; Start-Sleep -Seconds 30",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
        let (tx, commands) = async_channel::bounded(4);
        let (events, output) = async_channel::bounded(8);
        let (_frames, rx) = async_channel::bounded(4);
        tx.send(ClientCommand::Interrupt).await.unwrap();
        let mut session = Some(uuid::Uuid::new_v4().to_string());
        let original = session.clone();
        let result = run_turn(
            &mut child,
            Prompt {
                text: "test".into(),
                model: "default".into(),
                effort: None,
            },
            &mut session,
            &commands,
            &events,
            rx,
        )
        .await
        .unwrap();
        assert!(result);
        assert!(child.try_wait().unwrap().is_some());
        assert_eq!(session, original);
        assert!(matches!(output.recv().await.unwrap(), Event::Log(_)));
        assert!(matches!(output.recv().await.unwrap(),Event::Completed(s) if s=="interrupted"));
    }
}
