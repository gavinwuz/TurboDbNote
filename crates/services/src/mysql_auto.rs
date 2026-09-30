//! Peek at the bounded server greeting before authentication; never retry credentials or weaken TLS.
use super::*;
use tokio::io::AsyncReadExt;
fn version(greeting: &[u8]) -> Result<String, String> {
    if !matches!(greeting.first(), Some(9 | 10)) {
        return Err("MySQL 握手格式不受支持".into());
    }
    let rest = &greeting[1..];
    let end = rest
        .iter()
        .position(|b| *b == 0)
        .filter(|n| *n > 0 && *n <= 128)
        .ok_or("MySQL 版本信息无效")?;
    let value = std::str::from_utf8(&rest[..end]).map_err(|_| "MySQL 版本信息无效")?;
    if !value.as_bytes()[0].is_ascii_digit() {
        return Err("MySQL 版本信息无效".into());
    }
    Ok(value.into())
}
pub(super) fn legacy_target(version: &str) -> Option<&'static str> {
    if version.to_ascii_lowercase().contains("mariadb") {
        return None;
    }
    ["4.0", "4.1", "5.0", "5.1", "5.5"]
        .into_iter()
        .find(|prefix| version.starts_with(&format!("{prefix}.")) || version == *prefix)
}
async fn probe(profile: &Profile) -> Result<String, String> {
    let mut socket = tokio::net::TcpStream::connect((profile.host.as_str(), profile.port))
        .await
        .map_err(|_| "无法读取 MySQL 握手，请检查地址、端口或网络")?;
    let mut header = [0; 4];
    socket
        .read_exact(&mut header)
        .await
        .map_err(|_| "MySQL 握手读取失败")?;
    let length = header[0] as usize | (header[1] as usize) << 8 | (header[2] as usize) << 16;
    if header[3] != 0 || !(2..=4096).contains(&length) {
        return Err("MySQL 握手长度无效".into());
    }
    let mut body = vec![0; length];
    socket
        .read_exact(&mut body)
        .await
        .map_err(|_| "MySQL 握手不完整")?;
    version(&body)
}
pub(super) fn start(
    mut profile: Profile,
    password: String,
    test_only: bool,
) -> (Session, Receiver<Event>) {
    let (stop, rx) = async_channel::bounded(1);
    let (commands, requests) = async_channel::bounded(8);
    let (events, out) = async_channel::bounded(8);
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            let _ = events.send_blocking(Event::Failed("无法启动数据库后台任务".into()));
            return;
        };
        runtime.block_on(async move {
            let deadline=tokio::time::Instant::now()+Duration::from_secs(profile.timeout_seconds.clamp(1,60));
            let detected=tokio::select! { _=rx.recv()=>return, result=tokio::time::timeout(Duration::from_secs(profile.timeout_seconds.clamp(1,60)),probe(&profile))=>match result { Ok(result)=>result,Err(_)=>Err("MySQL 自动检测超时，已取消".into()) } };
            let version=match detected { Ok(v)=>v,Err(e)=>{let _=events.send(Event::Failed(e)).await;return;} };
            profile=detected_profile(profile,&version);
            if profile.driver==Driver::Legacy {
                if profile.legacy_library.is_empty() && let Some(path)=crate::mysql_drivers::bundled().filter(|p|crate::mysql_drivers::validate_pinned(p).is_ok()) { profile.legacy_library=path.display().to_string(); }
                if profile.tls != TlsMode::Disabled { let _=events.send(Event::Failed(TlsIssue::LegacyUnsupported.message().into())).await;return; }
                if let Err(error)=crate::mysql_legacy::validate(&profile) {
                    let message=if profile.legacy_library.is_empty() { "内置兼容组件不可用，请到设置 → 数据库 → 驱动修复组件".into() } else { error };
                    let _=events.send(Event::Failed(message)).await;return;
                }
            }
            let (session,received)=Session::start(profile,password,test_only);
            let timeout=tokio::time::sleep_until(deadline);tokio::pin!(timeout);let mut connected=false;
            loop { tokio::select! {
                _=&mut timeout, if !connected=>{let _=events.send(Event::Failed("MySQL 自动连接超时，已取消".into())).await;break;},
                _=rx.recv()=>break,
                request=requests.recv()=>match request {Ok(request)=>if session.request(request).is_err(){break;},Err(_)=>break},
                event=received.recv()=>match event {Ok(mut event)=>{
                    if let Event::Failed(message)=&mut event
                        && message.starts_with("该历史客户端不能可靠阻止") { *message="此兼容连接需要明确允许旧式密码认证，请在连接高级设置中确认；不会自动开启".into(); }
                    if matches!(&event,Event::Connected(_)|Event::Tested(_)){connected=true;}
                    if events.send(event).await.is_err(){break;}
                },Err(_)=>break},
            }}
        });
    });
    (Session::from_channels(stop, commands), out)
}
#[cfg(test)]
mod tests {
    #[test]
    fn legacy_probe_sends_no_credentials_and_preserves_tls_requirement() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let greeting = b"\x0a5.5.62\0";
            stream.write_all(&[greeting.len() as u8, 0, 0, 0]).unwrap();
            stream.write_all(greeting).unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        });
        let profile = super::Profile {
            host: "127.0.0.1".into(),
            port,
            database: "fixture".into(),
            ..Default::default()
        };
        let (_session, events) = super::start(profile, "fixture-secret".into(), true);
        let event = events.recv_blocking().unwrap();
        assert!(
            matches!(event,super::Event::Failed(message) if message==super::TlsIssue::LegacyUnsupported.message())
        );
        peer.join().unwrap();
    }
    #[test]
    fn routes_old_greetings_without_authentication_or_tls_changes() {
        for (v, legacy) in [
            ("4.0.30", Some("4.0")),
            ("4.1.22", Some("4.1")),
            ("5.5.62", Some("5.5")),
            ("5.6.51", None),
            ("8.4.0", None),
            ("5.5.5-10.6.0-MariaDB", None),
        ] {
            let greeting = [&[10][..], v.as_bytes(), &[0, 1, 2][..]].concat();
            let detected = super::version(&greeting).unwrap();
            assert_eq!(super::legacy_target(&detected), legacy);
        }
        assert!(super::version(&[255, 0]).is_err());
        assert!(super::version(&[10, 65, 0]).is_err());
    }
}
