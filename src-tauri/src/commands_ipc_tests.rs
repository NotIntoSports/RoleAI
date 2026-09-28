//! Exercise the generated IPC handler, not only synchronous service helpers.
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};
use tauri::{
    Manager,
    test::{mock_builder, mock_context, noop_assets},
};

use crate::{
    app_state::{AppPaths, AppState},
    secrets::MemorySecretStore,
};

fn state(root: &std::path::Path) -> AppState {
    AppState::initialize(
        AppPaths {
            data_directory: root.join("data"),
            logs_directory: root.join("logs"),
            config_path: root.join("config.json"),
            legacy_search_roots: vec![],
        },
        Arc::new(MemorySecretStore::default()),
    )
    .unwrap()
}

fn request(command: &str, body: serde_json::Value) -> tauri::webview::InvokeRequest {
    tauri::webview::InvokeRequest {
        cmd: command.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: "http://tauri.localhost".parse().unwrap(),
        body: tauri::ipc::InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.into(),
    }
}

#[test]
fn provider_discovery_ipc_returns_error_instead_of_panicking_in_async_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let app = mock_builder()
        .manage(state(directory.path()))
        .invoke_handler(tauri::generate_handler![super::model_provider_discover])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = tauri::test::get_ipc_response(
            &window,
            request(
                "model_provider_discover",
                serde_json::json!({ "providerId": "missing" }),
            ),
        )
        .map(|body| body.deserialize::<serde_json::Value>().unwrap());
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("provider IPC must resolve even when no provider exists")
        .unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "PROVIDER_NOT_FOUND");
    assert!(!app.state::<AppState>().session_control.is_cancelled());
}

#[test]
fn material_list_ipc_dispatch_does_not_wait_for_a_busy_database() {
    let directory = tempfile::tempdir().unwrap();
    let app = mock_builder()
        .manage(state(directory.path()))
        .invoke_handler(tauri::generate_handler![super::material_list])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let state = app.state::<AppState>();
    let database_guard = state.database.lock().unwrap();
    let (dispatched_tx, dispatched_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let dispatcher = std::thread::spawn(move || {
        window.on_message(
            request("material_list", serde_json::json!({})),
            Box::new(move |_, _, result, _, _| {
                let _ = result_tx.send(result);
            }),
        );
        let _ = dispatched_tx.send(());
    });
    let dispatched_without_lock = dispatched_rx
        .recv_timeout(Duration::from_millis(500))
        .is_ok();
    drop(database_guard);
    dispatcher.join().unwrap();
    let result = result_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        dispatched_without_lock,
        "IPC dispatch waited on the busy database instead of releasing the UI thread"
    );
    match result {
        tauri::ipc::InvokeResponse::Ok(body) => assert_eq!(
            body.deserialize::<serde_json::Value>().unwrap(),
            serde_json::json!({"ok": true, "data": []})
        ),
        tauri::ipc::InvokeResponse::Err(error) => panic!("unexpected IPC error: {error:?}"),
    }
}

fn invoke(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    tauri::test::get_ipc_response(window, request(command, body))
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap()
}

#[test]
fn slow_model_ipc_allows_takeover_and_stop_before_the_model_returns() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        time::Instant,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("model request did not arrive: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
            assert!(headers.len() < 16_384);
        }
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        assert!(length < 65_536);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        entered_tx
            .send((
                headers,
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            ))
            .unwrap();
        // Bounded even if a test assertion fails, so the client and test runtime can exit.
        let _ = release_rx.recv_timeout(Duration::from_secs(3));
        let body = r#"{"choices":[{"message":{"content":"controlled reply"}}]}"#;
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        socket.flush().unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    });
    let directory = tempfile::tempdir().unwrap();
    let mut config: serde_json::Value =
        serde_json::from_str(&super::tests::ready_session_config()).unwrap();
    for provider in config["models"]["providers"].as_array_mut().unwrap() {
        provider["baseUrl"] = serde_json::json!(endpoint);
    }
    std::fs::write(
        directory.path().join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let state = state(directory.path());
    for id in ["asr-1", "llm-1", "tts-1"] {
        state
            .secrets
            .set(&format!("providers/{id}/api-key"), "synthetic-ipc-test-key")
            .unwrap();
    }
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            super::session_start,
            super::session_finalize_utterance,
            super::session_stop,
            super::session_set_mode
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let started = invoke(&window, "session_start", serde_json::json!({}));
    assert_eq!(started["data"]["kind"], "started", "{started}");
    let (done_tx, done_rx) = mpsc::channel();
    let reply_window = window.clone();
    let turn = std::thread::spawn(move || {
        done_tx
            .send(invoke(
                &reply_window,
                "session_finalize_utterance",
                serde_json::json!({"text":"controlled question"}),
            ))
            .unwrap();
    });
    let (headers, request_body) = entered_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert_eq!(request_body["model"], "gpt");
    assert!(request_body["messages"].to_string().contains("PROMPT-BODY"));
    let start = Instant::now();
    let takeover = invoke(
        &window,
        "session_set_mode",
        serde_json::json!({"mode":"operator-speaking"}),
    );
    let stopped = invoke(&window, "session_stop", serde_json::json!({}));
    let elapsed = start.elapsed();
    let still_inflight = done_rx.try_recv().is_err();
    let cancelled = app.state::<AppState>().session_control.is_cancelled();
    release_tx.send(()).unwrap();
    let result = done_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    turn.join().unwrap();
    let listener = server.join().unwrap();
    assert!(
        elapsed < Duration::from_millis(500),
        "control IPC blocked for {elapsed:?}"
    );
    assert!(
        still_inflight,
        "control commands only returned after model completion"
    );
    assert!(cancelled);
    assert_eq!(takeover["ok"], true, "{takeover}");
    assert_eq!(stopped["data"]["status"], "stopping", "{stopped}");
    assert_eq!(result["error"]["code"], "SESSION_CANCELLED", "{result}");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "TTS must not be requested after cancellation"
    );
}

#[test]
fn diagnostics_export_rejects_destinations_outside_the_data_directory() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let app = mock_builder()
        .manage(state(directory.path()))
        .invoke_handler(tauri::generate_handler![super::diagnostics_export])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let escape_destination = outside.path().join("escape").join("report.json");
    std::fs::create_dir_all(outside.path().join("escape")).unwrap();
    let result = invoke(
        &window,
        "diagnostics_export",
        serde_json::json!({ "destination": escape_destination.to_string_lossy() }),
    );
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(
        result["error"]["code"], "DIAGNOSTICS_DESTINATION_INVALID",
        "{result}"
    );
    assert!(
        !escape_destination.exists(),
        "diagnostics export must not write outside the data directory"
    );

    let state = app.state::<AppState>();
    let allowed_directory = state.paths.data_directory.join("diagnostics");
    std::fs::create_dir_all(&allowed_directory).unwrap();
    let allowed_destination = allowed_directory.join("report.json");
    let result = invoke(
        &window,
        "diagnostics_export",
        serde_json::json!({ "destination": allowed_destination.to_string_lossy() }),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert!(
        allowed_destination.exists(),
        "in-data-directory export must still work"
    );
}
