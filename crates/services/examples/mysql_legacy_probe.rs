use turbodbn_services::mysql::{Driver, Event, Profile, Session, TlsMode};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let mode = args.get(3).map(String::as_str).unwrap_or("test");
    let profile = Profile {
        port: args[1].parse().unwrap(),
        legacy_library: args[2].clone(),
        driver: Driver::Legacy,
        tls: if mode == "tls-refuse" {
            TlsMode::Required
        } else {
            TlsMode::Disabled
        },
        charset: if ["gbk", "gb2312"].contains(&mode) {
            mode.into()
        } else {
            "server-default".into()
        },
        database: if mode == "single-db" {
            "测试".into()
        } else {
            String::new()
        },
        user: if mode == "old-auth" || mode == "old-auth-denied" {
            "legacy_probe".into()
        } else {
            "root".into()
        },
        allow_old_auth: mode != "old-auth-denied",
        timeout_seconds: 3,
        ..Default::default()
    };
    let password = if mode == "old-auth" || mode == "old-auth-denied" {
        "fixture-only"
    } else if mode == "wrong-password" {
        "wrong"
    } else {
        ""
    };
    let (session, events) = Session::start(profile, password.into(), mode != "connect");
    match events.recv_blocking().unwrap() {
        Event::Tested(info) | Event::Connected(info) => {
            assert!(!["wrong-password", "old-auth-denied", "tls-refuse"].contains(&mode));
            if ["gbk", "gb2312"].contains(&mode) {
                assert_eq!(info.charset, mode);
                assert!(
                    info.databases.contains(&"测试".into()),
                    "{:?}",
                    info.databases
                );
                assert!(info.database_list_error.is_none());
            }
            if mode == "single-db" {
                assert_eq!(info.databases, vec!["测试"]);
                assert_eq!(info.database.as_deref(), Some("测试"));
            }
            println!(
                "PASS: {mode} actual MySQL {}, charset {}",
                info.version, info.charset
            );
        }
        Event::Failed(error) => {
            match mode {
                "wrong-password" => assert!(error.contains("认证失败"), "{error}"),
                "old-auth-denied" => assert!(error.contains("旧密码认证"), "{error}"),
                "tls-refuse" => assert!(error.contains("TLS"), "{error}"),
                _ => panic!("{error}"),
            };
            println!("PASS: {mode} rejected");
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
    }
}
