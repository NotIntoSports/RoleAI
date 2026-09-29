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
    // 锁拆分后 stop 在收尾期间真正完成（此前是 pending 快路径）。
    assert_eq!(stopped["data"]["status"], "completed", "{stopped}");
    assert_eq!(result["error"]["code"], "SESSION_CANCELLED", "{result}");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "TTS must not be requested after cancellation"
    );
}

#[test]
fn slow_model_ipc_allows_stop_and_read_commands_while_finalize_is_inflight() {
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
                Ok((socket, _)) => {
                    break socket;
                }
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
        entered_tx.send(()).unwrap();
        // Bounded even if a test assertion fails, so the client and test runtime can exit.
        // 15s：红灯状态下 session_list 会被收尾阻塞数秒，释放窗口必须盖过它。
        let _ = release_rx.recv_timeout(Duration::from_secs(15));
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
            super::session_list
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
    entered_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    // 收尾（LLM 网络调用）进行中：停止与只读命令都必须在 500ms 内返回，
    // 不得排队到本轮回答结束（P2：三把全局锁全程持有）。
    let measure = |command: &'static str, body: serde_json::Value| {
        let control_window = window.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = invoke(&control_window, command, body);
            let _ = tx.send((start.elapsed(), result));
        });
        // 验收线是 500ms；这里用 2s 探针，超时即判红并点名被阻塞的命令。
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(pair) => pair,
            Err(_) => panic!(
                "{command} did not return within 2s while finalize was inflight (P2: blocked by finalize locks)"
            ),
        }
    };
    let start = Instant::now();
    let (stop_elapsed, stopped) = measure("session_stop", serde_json::json!({}));
    let (list_elapsed, listed) = measure("session_list", serde_json::json!({}));
    // 核心验收先断言：停止与只读命令不得被收尾的网络调用阻塞。
    assert!(
        stop_elapsed < Duration::from_millis(500),
        "session_stop blocked for {stop_elapsed:?} while finalize was inflight"
    );
    assert!(
        list_elapsed < Duration::from_millis(500),
        "session_list blocked for {list_elapsed:?} while finalize was inflight"
    );
    assert_eq!(stopped["ok"], true, "{stopped}");
    let stop_status = stopped["data"]["status"].as_str().unwrap_or_default();
    assert!(
        stop_status == "stopping" || stop_status == "completed",
        "unexpected stop status: {stopped}"
    );
    assert_eq!(listed["ok"], true, "{listed}");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "issuing both control commands took {:?}",
        start.elapsed()
    );
    release_tx.send(()).unwrap();
    let result = done_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    turn.join().unwrap();
    let listener = server.join().unwrap();
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

/// C12 锁拆分测试的共享脚手架：一次性 mock LLM 服务器。
/// 等首个 /v1/chat/completions 请求（entered 通知），收到 release（或超时）后
/// 返回受控回复。返回 (endpoint, entered, release, 服务器线程)。
fn slow_llm_server() -> (
    String,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
    std::thread::JoinHandle<std::net::TcpListener>,
) {
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
        entered_tx.send(()).unwrap();
        // 排水线程：LLM 请求挂起期间，preflight 等会向同一 mock 地址发探测请求；
        // 读掉并回 200，避免探测方等满自身超时。收到 stop 信号即退出。
        let drain_listener = listener.try_clone().unwrap();
        drain_listener.set_nonblocking(true).unwrap();
        // 排水线程常驻到进程退出（一次性测试服务器，进程结束随之消亡）。
        let _drain = std::thread::spawn(move || {
            loop {
                match drain_listener.accept() {
                    Ok((mut sock, _)) => {
                        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                        let mut reader = BufReader::new(sock.try_clone().unwrap());
                        let mut request_line = String::new();
                        if matches!(reader.read_line(&mut request_line), Ok(0) | Err(_)) {
                            continue;
                        }
                        // 按路径应答：embeddings 给最小合法向量；speech 给静音
                        // PCM（parse_tts_pcm 只要求非空偶数字节）；其余一律 500
                        // 快速失败，避免触发级联重试把测试拖过超时。
                        let mut header_line = String::new();
                        loop {
                            match reader.read_line(&mut header_line) {
                                Ok(0) | Err(_) => break,
                                Ok(_) if header_line == "\r\n" => break,
                                Ok(_) => header_line.clear(),
                            }
                        }
                        let (status, content_type, body): (&str, &str, Vec<u8>) =
                            if request_line.contains("/v1/embeddings") {
                                (
                                    "200 OK",
                                    "application/json",
                                    br#"{"data":[{"embedding":[0.0,0.0]}]}"#.to_vec(),
                                )
                            } else if request_line.contains("/v1/audio/speech") {
                                ("200 OK", "application/octet-stream", vec![0, 0, 0, 0])
                            } else {
                                (
                                    "500 Internal Server Error",
                                    "application/json",
                                    br#"{"error":{"message":"drained"}}"#.to_vec(),
                                )
                            };
                        let _ = write!(
                            sock,
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(&body);
                        let _ = sock.flush();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        // 有界等待：断言失败时客户端与测试运行时也能退出。
        let _ = release_rx.recv_timeout(Duration::from_secs(15));
        let body = r#"{"choices":[{"message":{"content":"controlled reply"}}]}"#;
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        socket.flush().unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    });
    (endpoint, entered_rx, release_tx, server)
}

/// C12 共享脚手架：把 ready_session_config 的全部供应商指向 mock endpoint，
/// 注入合成密钥，装配含会话与配置命令的 mock 应用。
fn finalize_test_app(
    directory: &std::path::Path,
    endpoint: &str,
) -> tauri::App<tauri::test::MockRuntime> {
    let mut config: serde_json::Value =
        serde_json::from_str(&super::tests::ready_session_config()).unwrap();
    for provider in config["models"]["providers"].as_array_mut().unwrap() {
        provider["baseUrl"] = serde_json::json!(endpoint);
    }
    std::fs::write(
        directory.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let state = state(directory);
    for id in ["asr-1", "llm-1", "tts-1"] {
        state
            .secrets
            .set(&format!("providers/{id}/api-key"), "synthetic-ipc-test-key")
            .unwrap();
    }
    mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            super::session_start,
            super::session_finalize_utterance,
            super::session_stop,
            super::session_list,
            super::session_get,
            super::config_get_public
        ])
        .build(mock_context(noop_assets()))
        .unwrap()
}

#[test]
fn stop_during_finalize_discards_the_turn_and_marks_the_session_finished() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, entered_rx, release_tx, server) = slow_llm_server();
    let app = finalize_test_app(directory.path(), &endpoint);
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let started = invoke(&window, "session_start", serde_json::json!({}));
    assert_eq!(started["data"]["kind"], "started", "{started}");
    let (done_tx, done_rx) = mpsc::channel();
    let reply_window = window.clone();
    let turn = std::thread::spawn(move || {
        let _invoked = invoke(
            &reply_window,
            "session_finalize_utterance",
            serde_json::json!({"text":"controlled question"}),
        );
        let _ = done_tx.send(_invoked);
    });
    entered_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("LLM request must arrive");
    // 收尾网络阶段（不持锁）中停止：stop 立即完成、本轮作废、绝不请求 TTS。
    let stopped = invoke(&window, "session_stop", serde_json::json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    release_tx.send(()).unwrap();
    let result = done_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    turn.join().unwrap();
    let listener = server.join().unwrap();
    assert_eq!(result["error"]["code"], "SESSION_CANCELLED", "{result}");
    // 会话被停止收尾；被取消的轮次不落库。
    let sessions = invoke(&window, "session_list", serde_json::json!({}));
    assert_eq!(sessions["ok"], true, "{sessions}");
    let rows = sessions["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{sessions}");
    assert_eq!(rows[0]["status"], "completed", "{sessions}");
    let id = rows[0]["id"].as_str().unwrap().to_owned();
    let detail = invoke(&window, "session_get", serde_json::json!({"sessionId": id}));
    assert_eq!(detail["ok"], true, "{detail}");
    assert_eq!(
        detail["data"]["turns"].as_array().unwrap().len(),
        0,
        "{detail}"
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "TTS must not be requested after cancellation"
    );
}

#[test]
fn stop_then_start_during_finalize_drops_the_inflight_turn() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, entered_rx, release_tx, server) = slow_llm_server();
    let app = finalize_test_app(directory.path(), &endpoint);
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let started = invoke(&window, "session_start", serde_json::json!({}));
    assert_eq!(started["data"]["kind"], "started", "{started}");
    let (done_tx, done_rx) = mpsc::channel();
    let reply_window = window.clone();
    let turn = std::thread::spawn(move || {
        let _invoked = invoke(
            &reply_window,
            "session_finalize_utterance",
            serde_json::json!({"text":"controlled question"}),
        );
        let _ = done_tx.send(_invoked);
    });
    entered_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("LLM request must arrive");
    // 收尾网络阶段：先停止旧会话，再开新会话。两者都不被收尾阻塞。
    let stopped = invoke(&window, "session_stop", serde_json::json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let restarted = invoke(&window, "session_start", serde_json::json!({}));
    assert_eq!(restarted["ok"], true, "{restarted}");
    assert_eq!(restarted["data"]["kind"], "started", "{restarted}");
    release_tx.send(()).unwrap();
    let result = done_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    turn.join().unwrap();
    server.join().unwrap();
    // 旧轮结果不得写进新会话：finalize 以“已取消”错误收场。
    assert_eq!(result["error"]["code"], "SESSION_CANCELLED", "{result}");
    let sessions = invoke(&window, "session_list", serde_json::json!({}));
    let rows = sessions["data"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{sessions}");
    // 新会话行状态在启动后保持 preparing（终态才改写），按“非旧会话”识别。
    let new_session = rows
        .iter()
        .find(|row| row["status"] != "completed")
        .expect("new session row must exist");
    let new_id = new_session["id"].as_str().unwrap().to_owned();
    let detail = invoke(
        &window,
        "session_get",
        serde_json::json!({"sessionId": new_id}),
    );
    assert_eq!(
        detail["data"]["turns"].as_array().unwrap().len(),
        0,
        "inflight turn must not leak into the new session: {detail}"
    );
    // 注：新会话重置了取消旗标，被弃轮次可能继续完成 TTS（排水线程应答），
    // 但结果必须整体丢弃，因此这里不断言 TTS 是否发生。
}

#[test]
fn config_reads_stay_responsive_and_the_turn_uses_the_phase1_snapshot() {
    use std::time::Instant;
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, entered_rx, release_tx, server) = slow_llm_server();
    let app = finalize_test_app(directory.path(), &endpoint);
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let started = invoke(&window, "session_start", serde_json::json!({}));
    assert_eq!(started["data"]["kind"], "started", "{started}");
    let (done_tx, done_rx) = mpsc::channel();
    let reply_window = window.clone();
    let turn = std::thread::spawn(move || {
        let _invoked = invoke(
            &reply_window,
            "session_finalize_utterance",
            serde_json::json!({"text":"controlled question"}),
        );
        let _ = done_tx.send(_invoked);
    });
    entered_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("LLM request must arrive");
    // 收尾网络阶段读配置：立即返回；本轮按阶段一快照执行。
    let start = Instant::now();
    let public = invoke(&window, "config_get_public", serde_json::json!({}));
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "config read blocked for {:?} while finalize was inflight",
        start.elapsed()
    );
    assert_eq!(public["ok"], true, "{public}");
    release_tx.send(()).unwrap();
    let result = done_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    turn.join().unwrap();
    server.join().unwrap();
    // 本轮按阶段一快照正常落库。
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(
        result["data"]["assistantText"], "controlled reply",
        "{result}"
    );
}
