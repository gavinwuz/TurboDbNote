//! Only for the isolated localhost server created by verify-mysql.ps1.
use turbodbn_services::mysql::{Event, Profile, Session, TlsMode};
fn main() {
    let port = std::env::args()
        .nth(1)
        .expect("mysql_probe <localhost port> <mode>")
        .parse()
        .unwrap();
    let mode = std::env::args().nth(2).unwrap_or("test".into());
    let profile = Profile {
        user: match mode.as_str() {
            "native-auth" => "probe_native",
            "sha2-auth" => "probe_sha2",
            _ => "root",
        }
        .into(),
        port,
        charset: if ["gbk", "gb2312"].contains(&mode.as_str()) {
            mode.clone()
        } else if mode == "custom-charset" {
            "latin1".into()
        } else {
            "utf8mb4".into()
        },
        tls: if mode == "tls-required" || mode == "sha2-auth" || mode == "tls-unsupported" {
            TlsMode::Required
        } else if mode == "tls-verify" {
            TlsMode::VerifyIdentity
        } else {
            TlsMode::Disabled
        },
        database: if mode == "single-db" {
            "测试库".into()
        } else if mode == "bad-db" {
            "does_not_exist".into()
        } else {
            String::new()
        },
        ..Default::default()
    };
    let (session, events) = Session::start(
        profile,
        if mode == "bad-password" {
            "wrong-password".into()
        } else if mode == "native-auth" || mode == "sha2-auth" {
            "fixture-only".into()
        } else {
            String::new()
        },
        mode != "connect",
    );
    match events.recv_blocking().unwrap() {
        Event::Tested(info) | Event::Connected(info) => {
            assert!(
                !["bad-password", "bad-db", "tls-verify", "tls-unsupported"]
                    .contains(&mode.as_str())
            );
            assert!(info.version.starts_with("8.0."));
            assert_eq!(
                info.charset,
                match mode.as_str() {
                    "gbk" => "gbk",
                    "gb2312" => "gb2312",
                    "custom-charset" => "latin1",
                    _ => "utf8mb4",
                }
            );
            if mode == "single-db" {
                assert_eq!(info.databases, vec!["测试库"]);
            }
            if ["gbk", "gb2312", "custom-charset"].contains(&mode.as_str()) {
                assert!(
                    info.databases.contains(&"测试库".into()),
                    "{:?}",
                    info.databases
                );
                assert!(info.database_list_error.is_none());
            }
            if mode == "tls-required" {
                assert!(!info.tls_cipher.is_empty());
            }
            println!("PASS: {mode} MySQL {} / {}", info.version, info.charset);
        }
        Event::Failed(error) => {
            let expected = match mode.as_str() {
                "bad-password" => "认证失败",
                "bad-db" => "默认数据库不存在",
                "tls-verify" => "证书",
                "tls-unsupported" => "未提供 TLS",
                _ => panic!("{error}"),
            };
            assert!(error.contains(expected), "Unexpected error: {error}");
            println!("PASS: {mode} correctly rejected");
        }
        Event::Operation { .. } => panic!("Unexpected operation event"),
        Event::Disconnected => panic!("Unexpected disconnect"),
    }
    drop(session);
    if mode == "connect" {
        assert!(matches!(
            events.recv_blocking().unwrap(),
            Event::Disconnected
        ));
        println!("PASS: explicit disconnect");
    }
}
