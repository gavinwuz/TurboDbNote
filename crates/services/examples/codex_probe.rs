//! Handshake probe. Optional --smoke-turn performs one minimal real model request.
use std::path::PathBuf;
use turbodbn_services::codex::{ClientCommand, Event, Prompt};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: codex_probe <absolute codex.exe path>");
    let engine = if std::env::args().any(|a| a == "--claude") {
        turbodbn_services::agent::Engine::Claude
    } else {
        turbodbn_services::agent::Engine::Codex
    };
    let (_connection, events) = turbodbn_services::agent::connect(
        engine,
        PathBuf::from(&path),
        std::env::current_dir().unwrap(),
    );
    let (mut account, mut models) = (false, false);
    let mut selected = None;
    let smoke = std::env::args().any(|arg| arg == "--smoke-turn");
    while let Ok(event) = events.recv_blocking() {
        match event {
            Event::Account { ready, .. } => {
                println!("Account ready: {ready}");
                account = true;
            }
            Event::Models(items) => {
                println!("Model catalog loaded: {} entries", items.len());
                models = true;
                selected = items
                    .iter()
                    .find(|m| m.is_default)
                    .or(items.first())
                    .cloned();
            }
            Event::Error(error) => panic!("Probe failed: {error}"),
            Event::Disconnected => panic!("Disconnected before handshake finished"),
            _ => {}
        }
        if account && models {
            println!("PASS: {} readiness and model choices", engine.name());
            break;
        }
    }
    let resume_model = selected.clone();
    let mut session = None;
    if smoke {
        let model = selected.expect("No models available");
        let effort = model
            .supported_reasoning_efforts
            .iter()
            .find(|e| e.reasoning_effort == "low")
            .map(|e| e.reasoning_effort.clone());
        _connection
            .send(ClientCommand::Send(Prompt {
                text: "Reply with exactly OK. Do not use any tools or read any files.".into(),
                model: model.model,
                effort,
            }))
            .unwrap();
        let mut received = false;
        while let Ok(event) = events.recv_blocking() {
            match event {
                Event::Thread(id) => session = Some(id),
                Event::Delta { text, .. } | Event::Snapshot { text, .. } => {
                    received |= !text.is_empty()
                }
                Event::Completed(status) => {
                    assert_eq!(status, "completed");
                    assert!(received);
                    println!("PASS: {} reply and completion", engine.name());
                    break;
                }
                Event::Error(error) => panic!("Turn probe failed: {error}"),
                Event::Disconnected => panic!("Disconnected during turn"),
                _ => {}
            }
        }
    }
    drop(_connection);
    // Wait for the worker to reap its child before this probe process exits.
    while let Ok(event) = events.recv_blocking() {
        if matches!(event, Event::Disconnected) {
            break;
        }
    }
    if std::env::args().any(|arg| arg == "--smoke-resume") {
        let id = session.expect("--smoke-resume requires --smoke-turn");
        let (connection, events) = turbodbn_services::agent::connect(
            engine,
            PathBuf::from(path),
            std::env::current_dir().unwrap(),
        );
        let (mut account, mut models) = (false, false);
        while let Ok(event) = events.recv_blocking() {
            match event {
                Event::Account { ready, .. } => {
                    assert!(ready);
                    account = true;
                }
                Event::Models(_) => models = true,
                Event::Error(error) => panic!("Resume connection failed: {error}"),
                Event::Disconnected => panic!("Disconnected before resume"),
                _ => {}
            }
            if account && models {
                break;
            }
        }
        connection
            .send(ClientCommand::RestoreThread(id.clone()))
            .unwrap();
        connection
            .send(ClientCommand::Send(Prompt {
                text: "Reply with exactly OK again. Do not use tools or read files.".into(),
                model: resume_model.unwrap().model,
                effort: None,
            }))
            .unwrap();
        let (mut reply, mut same_session) = (false, false);
        while let Ok(event) = events.recv_blocking() {
            match event {
                Event::Thread(restored) => same_session = restored == id,
                Event::Delta { text, .. } | Event::Snapshot { text, .. } => {
                    reply |= !text.is_empty()
                }
                Event::Completed(status) => {
                    assert_eq!(status, "completed");
                    assert!(reply && same_session);
                    println!("PASS: {} resumed the same session", engine.name());
                    break;
                }
                Event::Error(error) => panic!("Resume failed: {error}"),
                Event::Disconnected => panic!("Disconnected during resume"),
                _ => {}
            }
        }
        drop(connection);
        while let Ok(event) = events.recv_blocking() {
            if matches!(event, Event::Disconnected) {
                break;
            }
        }
    }
}
