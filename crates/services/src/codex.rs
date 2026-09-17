//! Codex App Server stdio adapter. UI never performs process I/O.
use async_channel::{Receiver, Sender};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const FRAME_LIMIT: usize = 1024 * 1024;
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(windows)]
pub(crate) mod job {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::*,
    };
    pub struct ProcessTree(HANDLE);
    impl ProcessTree {
        pub fn attach(child: &tokio::process::Child) -> Result<Self, String> {
            // SAFETY: valid process handle from Tokio; correctly-sized job limit structure.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let guard = Self(job);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let process = child.raw_handle().ok_or("AI process handle unavailable")?;
                if AssignProcessToJobObject(job, process as _) == 0 {
                    return Err(format!(
                        "无法管理 AI 子进程树：{}",
                        std::io::Error::last_os_error()
                    ));
                }
                Ok(guard)
            }
        }
    }
    impl Drop for ProcessTree {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub model: String,
    pub display_name: String,
    #[serde(default)]
    pub default_reasoning_effort: String,
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<Effort>,
    #[serde(default)]
    pub is_default: bool,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effort {
    pub reasoning_effort: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    pub text: String,
    pub model: String,
    pub effort: Option<String>,
}

pub enum ClientCommand {
    Send(Prompt),
    Interrupt,
    NewThread,
    RestoreThread(String),
}
#[derive(Debug)]
pub enum Event {
    Thread(String),
    Models(Vec<Model>),
    Account { ready: bool, label: String },
    Delta { item: String, text: String },
    Snapshot { item: String, text: String },
    Completed(String),
    Error(String),
    Log(String),
    Disconnected,
}

pub struct Connection {
    commands: Sender<ClientCommand>,
}
impl Connection {
    pub(crate) fn from_sender(commands: Sender<ClientCommand>) -> Self {
        Self { commands }
    }
    pub fn start(executable: PathBuf, cwd: PathBuf) -> (Self, Receiver<Event>) {
        let (commands, receiver) = async_channel::bounded(16);
        let (events, output) = async_channel::bounded(128);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(async {
                    if let Err(error) = run(executable, cwd, receiver, events.clone()).await {
                        let _ = events.send(Event::Error(error)).await;
                    }
                    let _ = events.send(Event::Disconnected).await;
                }),
                Err(error) => {
                    let _ = events.send_blocking(Event::Error(error.to_string()));
                }
            }
        });
        (Self { commands }, output)
    }
    pub fn send(&self, command: ClientCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| error.to_string())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.commands.close();
    }
}

enum Wire {
    Json(Value),
    Stderr,
    Closed(String),
}
async fn read_stream(reader: impl AsyncRead + Unpin, stderr: bool, tx: Sender<Wire>) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut bytes = Vec::new();
        let result = (&mut reader)
            .take((FRAME_LIMIT + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .await;
        let message = match result {
            Ok(0) => {
                if !stderr {
                    let _ = tx.send(Wire::Closed("AI 服务已退出".into())).await;
                }
                break;
            }
            Ok(_) if bytes.len() > FRAME_LIMIT => Wire::Closed("AI 协议帧超过 1 MiB 限制".into()),
            Ok(_) if stderr => Wire::Stderr, // Never forward raw diagnostics containing account/config data.
            Ok(_) => match serde_json::from_slice(&bytes) {
                Ok(value) => Wire::Json(value),
                Err(_) => Wire::Closed("AI 服务返回无效 JSON".into()),
            },
            Err(error) => Wire::Closed(format!("读取 AI 服务失败：{error}")),
        };
        let stop = matches!(message, Wire::Closed(_));
        if tx.send(message).await.is_err() || stop {
            break;
        }
    }
}

enum RpcKind {
    Init,
    Account,
    Models,
    Thread(Prompt),
    Turn,
    Interrupt,
}
struct Session {
    stdin: Box<dyn AsyncWrite + Unpin + Send>,
    next_id: u64,
    pending: HashMap<u64, (Instant, RpcKind)>,
    thread: Option<String>,
    turn: Option<String>,
    busy: bool,
    cancel: bool,
    models: Vec<Model>,
    ready: bool,
    cwd: PathBuf,
    turn_deadline: Option<Instant>,
}
impl Session {
    async fn write(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(5), self.stdin.write_all(&bytes))
            .await
            .map_err(|_| "向 AI 服务写入超时".to_string())?
            .map_err(|e| e.to_string())
    }
    async fn request(&mut self, method: &str, params: Value, kind: RpcKind) -> Result<(), String> {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        self.pending.insert(id, (Instant::now(), kind));
        Ok(())
    }
    async fn start_turn(&mut self, prompt: Prompt) -> Result<(), String> {
        self.request(
            "turn/start",
            json!({"threadId":self.thread,"model":prompt.model,
            "effort":prompt.effort,"approvalPolicy":"never",
            "sandboxPolicy":{"type":"readOnly"},
            "input":[{"type":"text","text":prompt.text}]}),
            RpcKind::Turn,
        )
        .await
    }
    async fn interrupt(&mut self) -> Result<(), String> {
        if let (Some(thread), Some(turn)) = (&self.thread, &self.turn) {
            self.request(
                "turn/interrupt",
                json!({"threadId":thread,"turnId":turn}),
                RpcKind::Interrupt,
            )
            .await?;
        }
        Ok(())
    }
}

async fn run(
    executable: PathBuf,
    cwd: PathBuf,
    commands: Receiver<ClientCommand>,
    events: Sender<Event>,
) -> Result<(), String> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err("请选择 Codex 可执行文件的绝对路径".into());
    }
    #[cfg(windows)]
    if !executable
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("请选择实际 codex.exe，不支持 .cmd / .ps1 启动脚本".into());
    }
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("工作目录不存在，请输入绝对路径".into());
    }
    let mut command = Command::new(&executable);
    command
        .arg("app-server")
        .arg("--listen")
        .arg("stdio://")
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 Codex：{e}"))?;
    #[cfg(windows)]
    let _process_tree = job::ProcessTree::attach(&child)?;
    let stdin = child.stdin.take().ok_or("Codex stdin 不可用")?;
    let (tx, wire) = async_channel::bounded(128);
    let stdout = child.stdout.take().ok_or("Codex stdout 不可用")?;
    let stderr = child.stderr.take().ok_or("Codex stderr 不可用")?;
    let out_task = tokio::spawn(read_stream(stdout, false, tx.clone()));
    let err_task = tokio::spawn(read_stream(stderr, true, tx));
    let mut session = Session {
        stdin: Box::new(stdin),
        next_id: 0,
        pending: HashMap::new(),
        thread: None,
        turn: None,
        busy: false,
        cancel: false,
        models: vec![],
        ready: false,
        cwd,
        turn_deadline: None,
    };
    let result = drive(&mut session, &mut child, commands, wire, &events).await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    out_task.abort();
    err_task.abort();
    result
}

async fn drive(
    s: &mut Session,
    child: &mut Child,
    commands: Receiver<ClientCommand>,
    wire: Receiver<Wire>,
    events: &Sender<Event>,
) -> Result<(), String> {
    s.request(
        "initialize",
        json!({"clientInfo":{"name":"turbodbnote","title":"TurboDbNote","version":"0.1.0"}}),
        RpcKind::Init,
    )
    .await?;
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let mut stderr_reported = false;
    let mut restore_thread: Option<String> = None;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Err(_) => return Ok(()),
                Ok(ClientCommand::NewThread) if !s.busy => { s.thread = None;restore_thread=None; }
                Ok(ClientCommand::NewThread) => {}
                Ok(ClientCommand::RestoreThread(id))=>{if !s.busy&&s.thread.is_none(){restore_thread=Some(id);}}
                Ok(ClientCommand::Interrupt) if s.busy => {
                    if !s.cancel { s.cancel = true; s.turn_deadline=Some(Instant::now()+Duration::from_secs(15)); s.interrupt().await?; }
                }
                Ok(ClientCommand::Interrupt) => {}
                Ok(ClientCommand::Send(prompt)) => {
                    if s.busy || !s.ready { events.send(Event::Error("AI 未就绪或正在生成".into())).await.map_err(|e| e.to_string())?; continue; }
                    if prompt.text.trim().is_empty() || prompt.text.len() > 65536 { return Err("输入必须为 1–65536 字节".into()); }
                    validate_model(&s.models, &prompt)?;
                    s.busy = true; s.cancel = false; s.turn = None;
                    s.turn_deadline = Some(Instant::now() + Duration::from_secs(300));
                    if s.thread.is_none() {
                        if let Some(id)=restore_thread.take(){
                            s.request("thread/resume",json!({"threadId":id,"cwd":s.cwd,"model":prompt.model,
                                "approvalPolicy":"never","sandbox":"read-only"}),RpcKind::Thread(prompt)).await?;
                        } else {
                        s.request("thread/start", json!({"cwd":s.cwd,"model":prompt.model,
                            "approvalPolicy":"never","sandbox":"read-only",
                            "baseInstructions":"You are TurboDbNote's SQL analysis assistant. Explain or propose SQL and Markdown. Never modify files or execute database statements. Do not request credentials."}), RpcKind::Thread(prompt)).await?;
                        }
                    } else { s.start_turn(prompt).await?; }
                    events.send(Event::Log("开始只读分析".into())).await.map_err(|e| e.to_string())?;
                }
            },
            message = wire.recv() => match message.map_err(|e| e.to_string())? {
                Wire::Closed(error) => return Err(error),
                Wire::Stderr => if !stderr_reported {
                    stderr_reported = true;
                    let _ = events.send(Event::Log("Codex 输出了诊断信息；原始诊断未写入聊天日志".into())).await;
                },
                Wire::Json(value) => handle_message(s, value, events).await?,
            },
            _ = ticker.tick() => {
                if events.is_closed() { return Ok(()); }
                if child.try_wait().map_err(|e| e.to_string())?.is_some() { return Err("Codex 进程已退出，请重新连接".into()); }
                if s.pending.values().any(|(at,_)| at.elapsed() > RPC_TIMEOUT) { return Err("Codex 协议请求超时，请重新连接".into()); }
                if s.turn_deadline.is_some_and(|at| Instant::now() > at) {
                    return Err(if s.cancel {"中断超过 15 秒，已终止后台服务"}else{"分析超过 5 分钟，已停止本次服务"}.into());
                }
            }
        }
    }
}

pub fn validate_model(models: &[Model], prompt: &Prompt) -> Result<(), String> {
    let model = models
        .iter()
        .find(|m| m.model == prompt.model)
        .ok_or("请选择当前可用模型")?;
    if let Some(effort) = &prompt.effort
        && !model
            .supported_reasoning_efforts
            .iter()
            .any(|e| &e.reasoning_effort == effort)
    {
        return Err("所选模型不支持该推理强度".into());
    }
    Ok(())
}

async fn handle_message(
    s: &mut Session,
    value: Value,
    events: &Sender<Event>,
) -> Result<(), String> {
    if value.get("method").is_some() && value.get("id").is_some() {
        // Never leave approval/tool requests waiting indefinitely in read-only mode.
        let method = value["method"].as_str().unwrap_or_default();
        let response = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                json!({"id":value["id"],"result":{"decision":"decline"}})
            }
            "execCommandApproval" | "applyPatchApproval" => {
                json!({"id":value["id"],"result":{"decision":"denied"}})
            }
            _ => {
                json!({"id":value["id"],"error":{"code":-32601,"message":"Unsupported in read-only analysis"}})
            }
        };
        s.write(response).await?;
        let _ = events
            .send(Event::Log("只读模式拒绝了工具授权请求".into()))
            .await;
        return Ok(());
    }
    if let Some(id) = value["id"].as_u64() {
        let Some((_, pending)) = s.pending.remove(&id) else {
            return Ok(());
        };
        if let Some(error) = value.get("error") {
            return Err(error["message"]
                .as_str()
                .unwrap_or("Codex 请求失败")
                .chars()
                .take(2000)
                .collect());
        }
        let result = &value["result"];
        match pending {
            RpcKind::Init => {
                s.write(json!({"method":"initialized","params":{}})).await?;
                s.request(
                    "account/read",
                    json!({"refreshToken":false}),
                    RpcKind::Account,
                )
                .await?;
                s.request("model/list", json!({"limit":100}), RpcKind::Models)
                    .await?;
            }
            RpcKind::Account => {
                s.ready = !result["account"].is_null() || result["requiresOpenaiAuth"] == false;
                let label = if s.ready {
                    "账号已就绪"
                } else {
                    "请先在 Codex 中登录，然后重新连接"
                };
                events
                    .send(Event::Account {
                        ready: s.ready,
                        label: label.into(),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            RpcKind::Models => {
                let models: Vec<Model> = serde_json::from_value(result["data"].clone())
                    .map_err(|e| format!("模型列表协议不兼容：{e}"))?;
                s.models.extend(models);
                if let Some(cursor) = result["nextCursor"].as_str() {
                    if s.models.len() > 500 {
                        return Err("模型列表超过上限".into());
                    }
                    s.request(
                        "model/list",
                        json!({"limit":100,"cursor":cursor}),
                        RpcKind::Models,
                    )
                    .await?;
                } else {
                    events
                        .send(Event::Models(s.models.clone()))
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            RpcKind::Thread(prompt) => {
                s.thread = Some(result["thread"]["id"].as_str().ok_or("缺少会话 ID")?.into());
                events
                    .send(Event::Thread(s.thread.clone().unwrap()))
                    .await
                    .map_err(|e| e.to_string())?;
                if s.cancel {
                    s.busy = false;
                    s.turn_deadline = None;
                    events
                        .send(Event::Completed("已停止".into()))
                        .await
                        .map_err(|e| e.to_string())?;
                } else {
                    s.start_turn(prompt).await?;
                }
            }
            RpcKind::Turn => {
                if !s.busy {
                    return Ok(());
                }
                s.turn = Some(result["turn"]["id"].as_str().ok_or("缺少请求 ID")?.into());
                if s.cancel {
                    s.interrupt().await?;
                }
            }
            RpcKind::Interrupt => {}
        }
        return Ok(());
    }
    let params = &value["params"];
    if params["threadId"].as_str() != s.thread.as_deref() || !s.busy {
        return Ok(());
    }
    if let (Some(incoming), Some(active)) = (
        params["turnId"].as_str().or(params["turn"]["id"].as_str()),
        s.turn.as_deref(),
    ) && incoming != active
    {
        return Ok(());
    }
    match value["method"].as_str().unwrap_or_default() {
        "item/agentMessage/delta" => {
            if let (Some(item), Some(text)) = (params["itemId"].as_str(), params["delta"].as_str())
            {
                events
                    .send(Event::Delta {
                        item: item.into(),
                        text: text.into(),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        "item/completed" if params["item"]["type"] == "agentMessage" => {
            if let (Some(item), Some(text)) = (
                params["item"]["id"].as_str(),
                params["item"]["text"].as_str(),
            ) {
                events
                    .send(Event::Snapshot {
                        item: item.into(),
                        text: text.into(),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        "turn/completed" => {
            let status = params["turn"]["status"].as_str().unwrap_or("completed");
            s.busy = false;
            s.turn = None;
            s.turn_deadline = None;
            events
                .send(Event::Completed(status.into()))
                .await
                .map_err(|e| e.to_string())?;
            if let Some(error) = params["turn"]["error"]["message"].as_str() {
                events
                    .send(Event::Error(error.chars().take(2000).collect()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Session {
        Session {
            stdin: Box::new(tokio::io::sink()),
            next_id: 0,
            pending: HashMap::new(),
            thread: Some("thread-a".into()),
            turn: Some("turn-new".into()),
            busy: true,
            cancel: false,
            models: vec![],
            ready: true,
            cwd: PathBuf::new(),
            turn_deadline: None,
        }
    }
    #[test]
    fn effort_must_belong_to_selected_model() {
        let models:Vec<Model>=serde_json::from_value(json!([{"id":"a","model":"a","displayName":"A","supportedReasoningEfforts":[{"reasoningEffort":"low"}]}])).unwrap();
        assert!(
            validate_model(
                &models,
                &Prompt {
                    text: "x".into(),
                    model: "a".into(),
                    effort: Some("high".into())
                }
            )
            .is_err()
        );
        assert!(
            validate_model(
                &models,
                &Prompt {
                    text: "x".into(),
                    model: "a".into(),
                    effort: Some("low".into())
                }
            )
            .is_ok()
        );
    }
    #[tokio::test]
    async fn stale_thread_and_turn_do_not_finish_current_turn() {
        let mut state = state();
        let (tx, rx) = async_channel::bounded(8);
        for params in [
            json!({"threadId":"thread-old","turn":{"id":"turn-new","status":"completed"}}),
            json!({"threadId":"thread-a","turn":{"id":"turn-old","status":"completed"}}),
        ] {
            handle_message(
                &mut state,
                json!({"method":"turn/completed","params":params}),
                &tx,
            )
            .await
            .unwrap();
        }
        assert!(state.busy);
        assert!(rx.is_empty());
        handle_message(&mut state,json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-new","status":"interrupted"}}}),&tx).await.unwrap();
        assert!(!state.busy);
        assert!(matches!(rx.recv().await.unwrap(), Event::Completed(_)));
    }
    #[tokio::test]
    async fn cancel_before_thread_response_never_starts_turn() {
        let mut state = state();
        state.cancel = true;
        state.pending.insert(
            7,
            (
                Instant::now(),
                RpcKind::Thread(Prompt {
                    text: "x".into(),
                    model: "a".into(),
                    effort: None,
                }),
            ),
        );
        let (tx, rx) = async_channel::bounded(8);
        handle_message(
            &mut state,
            json!({"id":7,"result":{"thread":{"id":"new-thread"}}}),
            &tx,
        )
        .await
        .unwrap();
        assert!(!state.busy);
        assert!(state.pending.is_empty());
        assert_eq!(state.next_id, 0);
        assert!(matches!(rx.recv().await.unwrap(),Event::Thread(id) if id=="new-thread"));
        assert!(matches!(rx.recv().await.unwrap(), Event::Completed(_)));
    }
    #[tokio::test]
    async fn completed_item_is_snapshot_not_duplicate_delta() {
        let mut state = state();
        let (tx, rx) = async_channel::bounded(8);
        handle_message(&mut state,json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-new","item":{"id":"i","type":"agentMessage","text":"hello"}}}),&tx).await.unwrap();
        assert!(matches!(rx.recv().await.unwrap(),Event::Snapshot{text,..} if text=="hello"));
    }
    #[tokio::test]
    async fn frame_reader_rejects_malformed_and_oversized_data() {
        for bytes in [b"not-json\n".to_vec(), vec![b'x'; FRAME_LIMIT + 1]] {
            let (tx, rx) = async_channel::bounded(4);
            read_stream(&bytes[..], false, tx).await;
            assert!(matches!(rx.recv().await.unwrap(), Wire::Closed(_)));
        }
    }
    #[tokio::test]
    async fn approvals_are_explicitly_declined() {
        let (mut capture, writer) = tokio::io::duplex(4096);
        let mut state = state();
        state.stdin = Box::new(writer);
        let (tx, _) = async_channel::bounded(8);
        handle_message(
            &mut state,
            json!({"id":"approval-1","method":"item/commandExecution/requestApproval","params":{}}),
            &tx,
        )
        .await
        .unwrap();
        let mut line = String::new();
        BufReader::new(&mut capture)
            .read_line(&mut line)
            .await
            .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], "approval-1");
        assert_eq!(response["result"]["decision"], "decline");
    }
}
