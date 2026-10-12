use crate::database::{ObjectItem, Operation, OperationKind, OperationResult};
// Legacy C client is loaded only by the helper process, never the GUI.
use crate::mysql::{Event, Profile, ServerInfo, Session, TlsMode};
use async_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString, c_char, c_uint, c_ulong, c_void},
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

#[derive(Serialize, Deserialize)]
struct Request {
    profile: Profile,
    password: String,
}
pub fn validate(profile: &Profile) -> Result<(), String> {
    if profile.tls != TlsMode::Disabled {
        return Err("兼容驱动当前仅支持显式禁用 TLS 的连接；请使用受保护网络/隧道，或使用现代驱动。不会自动降级。".into());
    }
    let library = Path::new(&profile.legacy_library);
    if !library.is_absolute() || !library.is_file() {
        return Err("请选择 MySQL 5.1–5.6 x64 libmysql.dll 的绝对路径".into());
    }
    if profile.charset == "utf8mb4" {
        return Err("历史客户端请选择 utf8；MySQL 4.0 请选择 server-default".into());
    }
    if profile.target_version == "4.0" && profile.charset != "server-default" {
        return Err("MySQL 4.0 不支持连接级 UTF-8 协商，请选择 server-default".into());
    }
    Ok(())
}
pub fn start(profile: Profile, password: String, test_only: bool) -> (Session, Receiver<Event>) {
    let (stop, rx) = async_channel::bounded(1);
    let (commands, requests) = async_channel::bounded::<Operation>(8);
    let (events, out) = async_channel::bounded(8);
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(async {
                if let Err(error) =
                    run(profile, password, test_only, rx, requests, events.clone()).await
                {
                    let _ = events.send(Event::Failed(error)).await;
                }
            }),
            Err(_) => {
                let _ = events.send_blocking(Event::Failed("无法启动兼容驱动任务".into()));
            }
        }
    });
    (Session::from_channels(stop, commands), out)
}
fn helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut base = executable.parent().ok_or("无法定位兼容进程")?.to_path_buf();
    if base
        .file_name()
        .is_some_and(|s| s == "examples" || s == "deps")
    {
        base.pop();
    }
    let helper = base.join(if cfg!(windows) {
        "turbodbnote-mysql-legacy.exe"
    } else {
        "turbodbnote-mysql-legacy"
    });
    if !helper.is_file() {
        return Err("缺少 turbodbnote-mysql-legacy 辅助程序，请构建/安装完整发行包".into());
    }
    Ok(helper)
}
async fn run(
    profile: Profile,
    password: String,
    test_only: bool,
    stop: Receiver<()>,
    requests: Receiver<Operation>,
    events: Sender<Event>,
) -> Result<(), String> {
    profile.validate()?;
    let _permit = if profile.kind == crate::DatabaseKind::External {
        Some(crate::external_driver::acquire()?)
    } else {
        None
    };
    let command = if profile.kind == crate::DatabaseKind::External {
        crate::external_driver::command(&profile)?
    } else {
        validate(&profile)?;
        tokio::process::Command::new(helper_path()?)
    };
    run_process(
        command, profile, password, test_only, stop, requests, events,
    )
    .await
}
async fn run_process(
    mut command: tokio::process::Command,
    profile: Profile,
    password: String,
    test_only: bool,
    stop: Receiver<()>,
    requests: Receiver<Operation>,
    events: Sender<Event>,
) -> Result<(), String> {
    let timeout = Duration::from_secs(profile.timeout_seconds.clamp(1, 60));
    let startup_timeout = timeout
        + if profile.kind == crate::DatabaseKind::External {
            Duration::from_secs(30)
        } else {
            Duration::ZERO
        };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|_| "无法启动兼容驱动辅助程序")?;
    #[cfg(windows)]
    let _tree = crate::codex::job::ProcessTree::attach(&child)?;
    let mut input = child.stdin.take().ok_or("兼容进程 stdin 不可用")?;
    let output = child.stdout.take().ok_or("兼容进程 stdout 不可用")?;
    let (handshake_done, mut handshake) = tokio::sync::oneshot::channel();
    let result = async {
        let mut data =
            serde_json::to_vec(&Request { profile, password }).map_err(|e| e.to_string())?;
        if data.len() > 65536 {
            return Err("兼容驱动连接参数过大".into());
        }
        data.push(b'\n');
        input
            .write_all(&data)
            .await
            .map_err(|_| "发送兼容驱动参数失败")?;
        data.fill(0);
        let mut output = tokio::io::BufReader::new(output);
        let mut line = String::new();
        (&mut output)
            .take(1048577)
            .read_line(&mut line)
            .await
            .map_err(|_| "读取兼容驱动结果失败")?;
        if line.len() > 1048576 {
            return Err("兼容驱动响应过大".into());
        }
        let value: serde_json::Value = serde_json::from_str(&line)
            .map_err(|_| "兼容驱动异常退出或返回无效响应（检查 DLL 位数和运行库）")?;
        if let Some(error) = value["error"].as_str() {
            return Err(error.chars().take(1500).collect());
        }
        let info: ServerInfo =
            serde_json::from_value(value["info"].clone()).map_err(|_| "兼容驱动响应格式错误")?;
        let _ = handshake_done.send(());
        if test_only {
            events
                .send(Event::Tested(info))
                .await
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        events
            .send(Event::Connected(info))
            .await
            .map_err(|e| e.to_string())?;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
        heartbeat.tick().await;
        loop {
            tokio::select! {
                _=stop.recv()=>{let _=events.send(Event::Disconnected).await;return Ok(());}
                request=requests.recv()=>{
                    let Ok(request)=request else{return Ok(());};let id=request.id;
                    let mut bytes=serde_json::to_vec(&request).map_err(|_|"操作参数无效")?;
                    if bytes.len()>65536 {let _=events.send(Event::Operation{id,result:Err("操作参数过大".into())}).await;continue;}
                    bytes.push(b'\n');input.write_all(&bytes).await.map_err(|_|"兼容进程已断开")?;
                    line.clear();
                    let reply=tokio::time::timeout(timeout,(&mut output).take(1048577).read_line(&mut line)).await;
                    if !matches!(reply,Ok(Ok(n)) if n>0) || line.len()>1048576 {
                        let _=events.send(Event::Operation{id,result:Err("操作超时或响应异常；建库结果可能已生效，请重连确认".into())}).await;
                        return Err("兼容操作连接已关闭".into());
                    }
                    let result=serde_json::from_str::<Result<OperationResult,String>>(&line).map_err(|_|"兼容操作响应无效")?;
                    let _=events.send(Event::Operation{id,result}).await;
                }
                _=heartbeat.tick()=>{
                    input.write_all(b"ping\n").await.map_err(|_|"兼容进程已断开")?;
                    line.clear();let n=tokio::time::timeout(Duration::from_secs(5),(&mut output).take(65537).read_line(&mut line)).await.map_err(|_|"兼容连接心跳超时")?.map_err(|_|"兼容连接读取失败")?;
                    if n==0||line.len()>65536||serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|v|v["ok"].as_bool())!=Some(true){return Err("兼容数据库连接已失效".into());}
                }
            }
        }
    };
    tokio::pin!(result);
    let deadline = tokio::time::sleep(startup_timeout);
    tokio::pin!(deadline);
    let mut waiting = true;
    let result = loop {
        tokio::select! {
            _=stop.recv()=>{if !test_only{let _=events.send(Event::Disconnected).await;}break Ok(());}
            result=&mut result=>break result,
            _=&mut handshake,if waiting=>{waiting=false;}
            _=&mut deadline,if waiting=>break Err("兼容驱动连接超时，已终止辅助进程".into()),
        }
    };
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

type Init = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type Close = unsafe extern "system" fn(*mut c_void);
type Options = unsafe extern "system" fn(*mut c_void, c_uint, *const c_void) -> i32;
type Connect = unsafe extern "system" fn(
    *mut c_void,
    *const c_char,
    *const c_char,
    *const c_char,
    *const c_char,
    c_uint,
    *const c_char,
    c_ulong,
) -> *mut c_void;
type Text = unsafe extern "system" fn(*mut c_void) -> *const c_char;
type Ping = unsafe extern "system" fn(*mut c_void) -> i32;
type Number = unsafe extern "system" fn(*mut c_void) -> c_uint;
type ThreadId = unsafe extern "system" fn(*mut c_void) -> c_ulong;
type Query = unsafe extern "system" fn(*mut c_void, *const c_char) -> i32;
type Store = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type Fetch = unsafe extern "system" fn(*mut c_void) -> *mut *mut c_char;
type ClientVersion = unsafe extern "system" fn() -> c_ulong;
struct Handle {
    raw: *mut c_void,
    close: Close,
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { (self.close)(self.raw) }
    }
}
fn c(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| "配置包含无效 NUL 字符".into())
}
unsafe fn text(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .chars()
            .take(4096)
            .collect()
    }
}
fn database_error(number: u32) -> String {
    if let Some(message) = crate::mysql::statement_error(number) {
        return message.into();
    }
    match number {
        1045 => "兼容驱动认证失败，请检查密码和主机授权".into(),
        1049 => "默认数据库不存在".into(),
        2049 => "服务器使用旧密码认证；请明确开启“允许旧认证”后重试".into(),
        n => format!("兼容驱动连接/查询失败（错误码 {n}）"),
    }
}

pub fn worker_main() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    std::io::Read::take(&mut reader, 65537)
        .read_line(&mut line)
        .map_err(|_| "无法读取连接参数")?;
    if line.len() > 65536 {
        return Err("连接参数过大".into());
    }
    let request: Request = serde_json::from_str(&line).map_err(|_| "无效连接参数")?;
    drop(line);
    request.profile.validate()?;
    validate(&request.profile)?;
    let begin = Instant::now();
    // SAFETY: this process exclusively loads an explicitly selected client DLL.
    // ABI is validated as MySQL 5.1–5.6 before any MYSQL handle is created.
    unsafe {
        #[cfg(windows)]
        let library: libloading::Library = libloading::os::windows::Library::load_with_flags(
            &request.profile.legacy_library,
            0x100 | 0x1000,
        )
        .map_err(|_| "无法加载兼容 DLL，请确认是 x64 MySQL 5.1–5.6 libmysql.dll 并安装其运行库")?
        .into();
        #[cfg(not(windows))]
        let library = libloading::Library::new(&request.profile.legacy_library)
            .map_err(|_| "无法加载兼容客户端库")?;
        macro_rules! symbol {
            ($name:literal,$ty:ty) => {
                *library
                    .get::<$ty>(concat!($name, "\0").as_bytes())
                    .map_err(|_| concat!("兼容库缺少 ", $name))?
            };
        }
        let client = symbol!("mysql_get_client_version", ClientVersion)();
        if !(50100..50700).contains(&client) {
            return Err(format!(
                "客户端版本 {client} 不在受支持的 5.1–5.6 ABI 范围；不要使用 MySQL 8 DLL"
            ));
        }
        if client < 50600 && !request.profile.allow_old_auth {
            return Err("该历史客户端不能可靠阻止服务器切换到旧密码认证；请选择 MySQL 5.6 客户端，或明确开启允许旧认证".into());
        }
        let init = symbol!("mysql_init", Init);
        let close = symbol!("mysql_close", Close);
        let options = symbol!("mysql_options", Options);
        let connect = symbol!("mysql_real_connect", Connect);
        let errno = symbol!("mysql_errno", Number);
        let raw = init(std::ptr::null_mut());
        if raw.is_null() {
            return Err("兼容客户端初始化失败".into());
        }
        let handle = Handle { raw, close };
        let timeout = request.profile.timeout_seconds as c_uint;
        let tcp: c_uint = 1;
        let disabled: c_uint = 0;
        for (option, value) in [
            (0, &timeout),
            (11, &timeout),
            (12, &timeout),
            (9, &tcp),
            (8, &disabled),
        ] {
            if options(raw, option, value as *const _ as _) != 0 {
                return Err("兼容驱动选项初始化失败".into());
            }
        }
        let secure: u8 = (!request.profile.allow_old_auth).into();
        let reconnect: u8 = 0;
        if options(raw, 18, &secure as *const _ as _) != 0
            || options(raw, 20, &reconnect as *const _ as _) != 0
        {
            return Err("兼容认证选项不受支持".into());
        }
        let charset = c("utf8")?;
        if request.profile.charset != "server-default"
            && options(raw, 7, charset.as_ptr() as _) != 0
        {
            return Err("客户端不支持所选字符集".into());
        }
        let host = c(&request.profile.host)?;
        let user = c(&request.profile.user)?;
        let password = c(&request.password)?;
        if connect(
            raw,
            host.as_ptr(),
            user.as_ptr(),
            password.as_ptr(),
            std::ptr::null(),
            request.profile.port as c_uint,
            std::ptr::null(),
            0,
        )
        .is_null()
        {
            return Err(database_error(errno(raw)));
        }
        let version = text(symbol!("mysql_get_server_info", Text)(raw));
        if !(version.starts_with("4.0.")
            || version.starts_with("4.1.")
            || version.starts_with("5."))
        {
            return Err(format!("兼容驱动连接到非目标版本 {version}，已关闭连接"));
        }
        if version.starts_with("4.0.") && request.profile.charset != "server-default" {
            return Err("MySQL 4.0 请使用 server-default 字符集".into());
        }
        let query = symbol!("mysql_query", Query);
        let original_charset = text(symbol!("mysql_character_set_name", Text)(raw));
        if !request.profile.database.is_empty() {
            let sql = format!("USE `{}`", request.profile.database.replace('`', "``"));
            let bytes = if version.starts_with("4.0.") {
                encode_name(&sql, &original_charset)?
            } else {
                if symbol!("mysql_set_character_set", Query)(raw, c("utf8")?.as_ptr()) != 0 {
                    return Err("无法设置数据库名称编码".into());
                }
                sql.into_bytes()
            };
            let sql = CString::new(bytes).map_err(|_| "数据库名无效")?;
            if query(raw, sql.as_ptr()) != 0 {
                return Err(database_error(errno(raw)));
            }
            if request.profile.charset == "server-default"
                && !version.starts_with("4.0.")
                && symbol!("mysql_set_character_set", Query)(raw, c(&original_charset)?.as_ptr())
                    != 0
            {
                return Err("恢复默认字符集失败".into());
            }
        }
        if request.profile.charset != "server-default" {
            let charset = c(&request.profile.charset)?;
            if symbol!("mysql_set_character_set", Query)(raw, charset.as_ptr()) != 0 {
                return Err(format!(
                    "服务器或客户端不支持字符集 {}",
                    request.profile.charset
                ));
            }
        }
        let charset_name = text(symbol!("mysql_character_set_name", Text)(raw));
        let unicode_metadata = !version.starts_with("4.0.");
        if unicode_metadata && query(raw, c("SET character_set_results = utf8")?.as_ptr()) != 0 {
            return Err("无法设置数据库名称解码方式".into());
        }
        let store = symbol!("mysql_store_result", Store);
        let fetch = symbol!("mysql_fetch_row", Fetch);
        let free = symbol!("mysql_free_result", Close);
        if query(
            raw,
            c("SELECT VERSION(), CONNECTION_ID(), DATABASE()")?.as_ptr(),
        ) != 0
        {
            return Err(database_error(errno(raw)));
        }
        let result = store(raw);
        if result.is_null() {
            return Err("兼容服务器未返回检测结果".into());
        }
        let result_guard = Handle {
            raw: result,
            close: free,
        };
        if symbol!("mysql_num_fields", Number)(result) != 3 {
            return Err("兼容服务器返回的检测列数错误".into());
        }
        let row = fetch(result);
        if row.is_null() {
            return Err("兼容服务器未返回检测行".into());
        }
        let database = if (*row.add(2)).is_null() {
            None
        } else {
            Some(decode_name(
                CStr::from_ptr(*row.add(2)).to_bytes(),
                if unicode_metadata {
                    "utf8"
                } else {
                    &charset_name
                },
            )?)
        };
        drop(result_guard);
        let mut databases = Vec::new();
        let mut database_list_error = None;
        if !request.profile.database.is_empty() && !request.profile.list_all_databases {
            databases.push(request.profile.database.clone());
        } else if query(raw, c("SHOW DATABASES")?.as_ptr()) != 0 {
            database_list_error =
                Some("无法读取数据库列表，请检查账号权限；仍可手动输入库名".into());
        } else {
            let rows = symbol!("mysql_use_result", Store)(raw);
            if rows.is_null() {
                database_list_error = Some("数据库列表读取失败".into());
            } else {
                let rows_guard = Handle {
                    raw: rows,
                    close: free,
                };
                if symbol!("mysql_num_fields", Number)(rows) != 1 {
                    return Err("数据库列表列数错误".into());
                }
                let mut seen = 0;
                loop {
                    let row = fetch(rows);
                    if row.is_null() {
                        break;
                    }
                    if seen == 512 {
                        database_list_error =
                            Some("数据库超过 512 个，请直接输入库名缩小范围".into());
                        break;
                    }
                    seen += 1;
                    if !(*row).is_null() {
                        match decode_name(
                            CStr::from_ptr(*row).to_bytes(),
                            if unicode_metadata {
                                "utf8"
                            } else {
                                &charset_name
                            },
                        ) {
                            Ok(name) => databases.push(name),
                            Err(error) => {
                                database_list_error =
                                    Some(format!("已跳过无效库名：{error}，请检查服务端命名编码"));
                            }
                        }
                    }
                }
                drop(rows_guard);
            }
        }
        if unicode_metadata {
            if !crate::mysql::valid_charset(&charset_name) {
                return Err("服务器字符集名称无效".into());
            }
            if query(
                raw,
                c(&format!("SET character_set_results = {charset_name}"))?.as_ptr(),
            ) != 0
            {
                return Err("恢复连接字符集失败".into());
            }
        }
        let info = ServerInfo {
            version,
            databases,
            database_list_error,
            session_id: symbol!("mysql_thread_id", ThreadId)(raw) as u64,
            charset: charset_name,
            database,
            tls_cipher: String::new(),
            elapsed_ms: begin.elapsed().as_millis(),
        };
        println!("{}", serde_json::json!({"info":info}));
        std::io::stdout().flush().map_err(|_| "发送连接结果失败")?;
        let ping = symbol!("mysql_ping", Ping);
        loop {
            line = String::new();
            let n = std::io::Read::take(&mut reader, 65537)
                .read_line(&mut line)
                .map_err(|_| "控制通道断开")?;
            if n == 0 {
                break;
            }
            if line.len() > 65536 {
                return Err("兼容控制命令过大".into());
            }
            if line.trim() == "ping" {
                println!("{}", serde_json::json!({"ok":ping(raw)==0}));
            } else {
                let operation: Operation =
                    serde_json::from_str(&line).map_err(|_| "无效兼容控制命令")?;
                let result = legacy_operation(
                    raw,
                    &library,
                    &request.profile,
                    &info.version,
                    &info.charset,
                    &operation,
                );
                let encoded = serde_json::to_string(&result).map_err(|_| "操作响应编码失败")?;
                if encoded.len() > 1024 * 1024 {
                    println!(
                        "{}",
                        serde_json::to_string(&Err::<OperationResult, _>(
                            "元数据响应过大，请缩小范围"
                        ))
                        .unwrap()
                    );
                } else {
                    println!("{encoded}");
                }
            }
            std::io::stdout().flush().map_err(|_| "控制通道断开")?;
        }
        drop(handle);
        drop(library);
    }
    Ok(())
}

unsafe fn legacy_rows(
    raw: *mut c_void,
    library: &libloading::Library,
    sql: &str,
    charset: &str,
) -> Result<Vec<Vec<String>>, String> {
    unsafe {
        let query = *library
            .get::<Query>(b"mysql_query ")
            .map_err(|_| "缺少查询接口")?;
        let sql = CString::new(encode_name(sql, charset)?).map_err(|_| "对象名称无效")?;
        if query(raw, sql.as_ptr()) != 0 {
            let errno = *library
                .get::<Number>(b"mysql_errno ")
                .map_err(|_| "缺少错误接口")?;
            return Err(database_error(errno(raw)));
        }
        let result = (*library
            .get::<Store>(b"mysql_use_result ")
            .map_err(|_| "缺少读取接口")?)(raw);
        if result.is_null() {
            return Ok(Vec::new());
        }
        let free = *library
            .get::<Close>(b"mysql_free_result ")
            .map_err(|_| "缺少释放接口")?;
        let _guard = Handle {
            raw: result,
            close: free,
        };
        let count = (*library
            .get::<Number>(b"mysql_num_fields ")
            .map_err(|_| "缺少列接口")?)(result) as usize;
        if count > 32 {
            return Err("元数据列数超限".into());
        }
        let fetch = *library
            .get::<Fetch>(b"mysql_fetch_row ")
            .map_err(|_| "缺少行接口")?;
        let mut rows = Vec::new();
        while rows.len() < 513 {
            let row = fetch(result);
            if row.is_null() {
                break;
            }
            let mut values = Vec::new();
            for i in 0..count {
                let value = *row.add(i);
                values.push(if value.is_null() {
                    String::new()
                } else {
                    decode_name(CStr::from_ptr(value).to_bytes(), charset)?
                        .chars()
                        .take(2048)
                        .collect()
                });
            }
            rows.push(values);
        }
        Ok(rows)
    }
}
unsafe fn legacy_operation(
    raw: *mut c_void,
    library: &libloading::Library,
    p: &Profile,
    version: &str,
    charset: &str,
    op: &Operation,
) -> Result<OperationResult, String> {
    unsafe {
        let unicode = !version.starts_with("4.0.");
        let set = *library
            .get::<Query>(b"mysql_set_character_set ")
            .map_err(|_| "缺少字符集接口")?;
        if unicode && set(raw, c("utf8")?.as_ptr()) != 0 {
            return Err("元数据字符集切换失败".into());
        }
        let encoding = if unicode { "utf8" } else { charset };
        let result = (|| {
            use crate::database::{objects, quote_mysql};
            match &op.kind {
                OperationKind::CopyDatabase {
                    source,
                    target,
                    charset,
                    include_data,
                } => {
                    use futures_util::FutureExt;
                    let mut access = LegacyAccess {
                        raw,
                        library,
                        charset: encoding,
                    };
                    crate::database::copy::mysql(
                        &mut access,
                        source,
                        target,
                        charset,
                        *include_data,
                    )
                    .now_or_never()
                    .ok_or("兼容复制意外等待异步操作")?
                }
                OperationKind::Workbench(r) => {
                    use futures_util::FutureExt;
                    let mut access = LegacyAccess {
                        raw,
                        library,
                        charset: encoding,
                    };
                    crate::database::workbench::run(&mut access, p.kind, version, r)
                        .now_or_never()
                        .ok_or("兼容驱动意外等待异步操作")?
                        .map(|r| OperationResult::Workbench(Box::new(r)))
                }
                OperationKind::Databases => Ok(OperationResult::Databases(
                    if p.database.is_empty() || p.list_all_databases {
                        legacy_rows(raw, library, "SHOW DATABASES", encoding)?
                            .into_iter()
                            .take(512)
                            .filter_map(|r| r.into_iter().next())
                            .collect()
                    } else {
                        vec![p.database.clone()]
                    },
                )),
                OperationKind::CreateDatabase {
                    name,
                    charset,
                    collation,
                } => {
                    if version.starts_with("4.0") && (!charset.is_empty() || !collation.is_empty())
                    {
                        return Err("MySQL 4.0 仅支持服务器默认字符集".into());
                    }
                    legacy_rows(
                        raw,
                        library,
                        &crate::database::create_database_sql_with_collation(
                            p.kind, name, charset, collation,
                        )?,
                        encoding,
                    )?;
                    Ok(OperationResult::Created(name.clone()))
                }
                OperationKind::Objects { database, .. } => {
                    let full = version.starts_with("5.");
                    let sql = format!(
                        "SHOW {}TABLES FROM {}",
                        if full { "FULL " } else { "" },
                        quote_mysql(database)
                    );
                    let rows = legacy_rows(raw, library, &sql, encoding)?;
                    Ok(objects(
                        rows.into_iter()
                            .filter(|r| !r.is_empty())
                            .map(|r| {
                                ObjectItem::new(
                                    String::new(),
                                    r[0].clone(),
                                    if r.get(1).is_some_and(|s| s == "VIEW") {
                                        "视图"
                                    } else {
                                        "表"
                                    }
                                    .into(),
                                )
                            })
                            .collect(),
                    ))
                }
                OperationKind::Details { database, object } => {
                    if object.kind == "database" {
                        let rows = legacy_rows(
                            raw,
                            library,
                            "SHOW VARIABLES LIKE 'character_set_database'",
                            encoding,
                        )?;
                        return Ok(OperationResult::Details(vec![(
                            "charset".into(),
                            rows.first()
                                .and_then(|r| r.get(1))
                                .cloned()
                                .unwrap_or_else(|| "server-default".into()),
                        )]));
                    }
                    let qualified =
                        format!("{}.{}", quote_mysql(database), quote_mysql(&object.name));
                    let mut details = Vec::new();
                    for r in legacy_rows(
                        raw,
                        library,
                        &format!("SHOW FULL COLUMNS FROM {qualified}"),
                        encoding,
                    )? {
                        if !r.is_empty() {
                            details.push((format!("列 · {}", r[0]), r[1..].join(" · ")));
                        }
                    }
                    for r in legacy_rows(
                        raw,
                        library,
                        &format!("SHOW INDEX FROM {qualified}"),
                        encoding,
                    )? {
                        if r.len() > 4 {
                            details.push((
                                format!("索引 · {}", r[2]),
                                format!("列 {} · 非唯一 {}", r[4], r[1]),
                            ));
                        }
                    }
                    Ok(OperationResult::Details(details))
                }
            }
        })();
        if unicode && set(raw, c(charset)?.as_ptr()) != 0 {
            return Err("恢复连接字符集失败，请重连".into());
        }
        result
    }
}

fn encode_name(value: &str, charset: &str) -> Result<Vec<u8>, String> {
    if value.is_ascii() {
        return Ok(value.as_bytes().to_vec());
    }
    let encoding = match charset.to_ascii_lowercase().as_str() {
        "utf8" | "utf8mb4" => Some(encoding_rs::UTF_8),
        "gbk" | "gb2312" => Some(encoding_rs::GBK),
        "latin1" => Some(encoding_rs::WINDOWS_1252),
        other => encoding_rs::Encoding::for_label(other.as_bytes()),
    }
    .ok_or("不支持当前服务器数据库名编码")?;
    let (bytes, _, errors) = encoding.encode(value);
    if errors {
        Err("数据库名无法用服务器默认字符集表示".into())
    } else {
        Ok(bytes.into_owned())
    }
}
fn decode_name(bytes: &[u8], charset: &str) -> Result<String, String> {
    if bytes.is_ascii() {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    }
    let encoding = match charset.to_ascii_lowercase().as_str() {
        "utf8" | "utf8mb4" => Some(encoding_rs::UTF_8),
        "gbk" | "gb2312" => Some(encoding_rs::GBK),
        "latin1" => Some(encoding_rs::WINDOWS_1252),
        other => encoding_rs::Encoding::for_label(other.as_bytes()),
    }
    .ok_or("无法解码服务器数据库名称，请使用支持的字符集")?;
    let (value, _, errors) = encoding.decode(bytes);
    if errors {
        Err("数据库名称编码无效，未使用乱码名称".into())
    } else {
        Ok(value.into_owned())
    }
}

struct LegacyAccess<'a> {
    raw: *mut c_void,
    library: &'a libloading::Library,
    charset: &'a str,
}
impl crate::database::workbench::Access for LegacyAccess<'_> {
    fn query<'a>(&'a mut self, sql: &'a str) -> crate::database::workbench::Query<'a> {
        Box::pin(async move {
            unsafe { legacy_workbench_rows(self.raw, self.library, sql, self.charset) }
        })
    }
}
unsafe fn legacy_workbench_rows(
    raw: *mut c_void,
    library: &libloading::Library,
    sql: &str,
    charset: &str,
) -> Result<crate::database::workbench::Rows, String> {
    unsafe {
        let query = *library
            .get::<Query>(b"mysql_query\0")
            .map_err(|_| "缺少查询接口")?;
        let sql = CString::new(encode_name(sql, charset)?).map_err(|_| "SQL 包含空字符")?;
        if query(raw, sql.as_ptr()) != 0 {
            let errno = *library
                .get::<Number>(b"mysql_errno\0")
                .map_err(|_| "缺少错误接口")?;
            return Err(database_error(errno(raw)));
        }
        let result = (*library
            .get::<Store>(b"mysql_use_result\0")
            .map_err(|_| "缺少读取接口")?)(raw);
        let mut out = crate::database::workbench::Rows::default();
        if result.is_null() {
            return Ok(out);
        }
        let free = *library
            .get::<Close>(b"mysql_free_result\0")
            .map_err(|_| "缺少释放接口")?;
        let _guard = Handle {
            raw: result,
            close: free,
        };
        let count = (*library
            .get::<Number>(b"mysql_num_fields\0")
            .map_err(|_| "缺少列接口")?)(result) as usize;
        if count > 512 {
            return Err("列数超过 512".into());
        }
        let fetch = *library
            .get::<Fetch>(b"mysql_fetch_row\0")
            .map_err(|_| "缺少行接口")?;
        type Lengths = unsafe extern "C" fn(*mut c_void) -> *mut std::ffi::c_ulong;
        let lengths = *library
            .get::<Lengths>(b"mysql_fetch_lengths\0")
            .map_err(|_| "缺少长度接口")?;
        let mut bytes = 0;
        loop {
            let row = fetch(result);
            if row.is_null() {
                break;
            }
            let lens = lengths(result);
            if lens.is_null() {
                return Err("读取字段长度失败".into());
            }
            let mut values = Vec::new();
            for i in 0..count {
                let v = *row.add(i);
                let value = if v.is_null() {
                    None
                } else {
                    let b = std::slice::from_raw_parts(v as *const u8, *lens.add(i) as usize);
                    Some(decode_name(b, charset).unwrap_or_else(|_| {
                        format!(
                            "0x{}",
                            b.iter().map(|v| format!("{v:02x}")).collect::<String>()
                        )
                    }))
                };
                bytes += value.as_ref().map_or(0, |s| s.len());
                values.push(value);
            }
            if bytes > crate::database::workbench::MAX_BYTES || out.values.len() >= 2000 {
                return Err("结果超过预览上限（768 KiB）".into());
            }
            out.values.push(values);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sql_syntax_and_missing_table_have_shared_sanitized_categories() {
        assert_eq!(database_error(1064), crate::mysql::SQL_SYNTAX_ERROR);
        assert_eq!(database_error(1146), crate::mysql::TABLE_MISSING);
        assert_eq!(crate::mysql::statement_error(1142), None);
    }
    #[test]
    fn chinese_database_names_decode_without_loss() {
        let (bytes, _, _) = encoding_rs::GBK.encode("测试数据库");
        assert_eq!(decode_name(&bytes, "gbk").unwrap(), "测试数据库");
        assert!(decode_name(&[0xff], "utf8").is_err());
    }
    #[test]
    fn legacy_never_silently_downgrades_tls() {
        let profile = Profile {
            driver: crate::mysql::Driver::Legacy,
            ..Default::default()
        };
        assert!(validate(&profile).unwrap_err().contains("TLS"));
    }
    #[test]
    fn legacy_requires_explicit_library_and_compatible_charset() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut p = Profile {
            driver: crate::mysql::Driver::Legacy,
            tls: TlsMode::Disabled,
            legacy_library: file.path().display().to_string(),
            charset: "utf8".into(),
            ..Default::default()
        };
        assert!(validate(&p).is_ok());
        p.target_version = "4.0".into();
        assert!(validate(&p).is_err());
        p.charset = "server-default".into();
        assert!(validate(&p).is_ok());
        p.legacy_library = "relative.dll".into();
        assert!(validate(&p).is_err());
    }
    #[test]
    fn legacy_routes_are_selected_without_auth_fallback() {
        for target in ["4.0", "4.1", "5.0", "5.1", "5.5", "5.0–5.5"] {
            let p = Profile {
                target_version: target.into(),
                ..Default::default()
            };
            assert!(crate::mysql::uses_legacy(&p));
        }
        let p = Profile {
            target_version: "5.7".into(),
            ..Default::default()
        };
        assert!(!crate::mysql::uses_legacy(&p));
    }
}
