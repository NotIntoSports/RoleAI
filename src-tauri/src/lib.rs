use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

mod app_state;
pub mod audio;
mod commands;
pub mod config;
pub mod contracts;
pub mod database;
pub mod diagnostics;
pub mod error;
pub mod livestream;
pub mod materials;
pub mod migrate;
pub mod obs;
pub mod prerequisites;
pub mod processes;
pub mod providers;
pub mod runtime;
pub mod secrets;
pub mod services;
pub mod sessions;
pub mod startup;

fn navigation_is_allowed(url: &tauri::Url) -> bool {
    if url.scheme() == "tauri" {
        return true;
    }

    // Tauri v2 serves the packaged Windows app from http://tauri.localhost
    // (WebView2). Allow exactly that origin — http scheme, exact host, no port —
    // so a release build does not white-screen. This is fail-closed: a lookalike
    // host such as tauri.localhost.evil.com, any port, or https is rejected below.
    if url.scheme() == "http" && url.host_str() == Some("tauri.localhost") && url.port().is_none() {
        return true;
    }

    cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if startup::is_isolation_support_check(&arguments) {
        print!("{}", startup::ISOLATION_SUPPORT_MARKER);
        return;
    }
    let isolated = match startup::parse_isolated_startup(
        &arguments,
        std::env::var_os("AI_VIRTUAL_ASSISTANT_CONFIG").as_deref(),
    ) {
        Ok(isolated) => isolated,
        Err(error) => {
            eprintln!("isolated startup rejected: {error}");
            std::process::exit(2);
        }
    };
    if let Some(isolated) = isolated.as_ref()
        && let Err(error) = startup::validate_isolated_webview_environment(
            isolated,
            &std::env::vars_os().collect::<Vec<_>>(),
        )
    {
        eprintln!("isolated startup rejected: {error}");
        std::process::exit(2);
    }

    let builder = tauri::Builder::default().plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _, event| {
                if event.state() == ShortcutState::Pressed {
                    let _ = app.emit("session:assistant_hotkey:v1", ());
                }
            })
            .build(),
    );
    let builder = if isolated.is_none() {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
    } else {
        builder
    };
    builder
        .invoke_handler(tauri::generate_handler![
            commands::foundation_get_status,
            commands::diagnostics_export,
            commands::config_get_startup_state,
            commands::legacy_migration_status,
            commands::legacy_import_source,
            commands::config_get_public,
            commands::model_provider_save,
            commands::model_provider_test,
            commands::model_provider_discover,
            commands::model_provider_activate,
            commands::model_provider_delete,
            commands::model_provider_dependencies,
            commands::speech_route_save,
            commands::speech_route_test,
            commands::speech_route_activate,
            commands::speech_route_delete,
            commands::voice_reference_save,
            commands::voice_reference_save_audio,
            commands::voice_reference_update,
            commands::voice_reference_list,
            commands::voice_reference_delete,
            commands::voice_reference_clone,
            commands::role_profile_save,
            commands::role_profile_copy,
            commands::role_profile_activate,
            commands::role_profile_delete,
            commands::embedding_config_save,
            commands::embedding_config_test,
            commands::embedding_config_activate,
            commands::embedding_config_delete,
            commands::material_list,
            commands::material_import,
            commands::material_search,
            commands::material_delete,
            commands::material_index,
            commands::session_start,
            commands::session_stop,
            commands::session_set_mode,
            commands::session_export,
            commands::session_list,
            commands::session_get,
            commands::session_delete,
            commands::session_finalize_utterance,
            commands::session_trigger_assistant,
            commands::session_agent_command,
            commands::runtime_get_status,
            commands::config_restore_last_good,
            commands::config_restore_defaults,
            commands::open_app_directory,
            commands::open_web_source,
            commands::meeting_process_list,
            commands::audio_output_list,
            commands::virtual_audio_status,
            commands::virtual_audio_install,
            commands::session_audio_ready,
            commands::session_push_mic_pcm,
            commands::session_push_video_frame,
            commands::livestream_create_draft,
            commands::livestream_generate,
            commands::livestream_get,
            commands::livestream_control,
            commands::livestream_insert_question,
            commands::obs_runtime_status,
            commands::obs_virtual_camera_start,
            commands::obs_virtual_camera_stop,
            commands::obs_password_status,
            commands::obs_password_save,
        ])
        .setup(move |app| {
            if let Some(isolated) = isolated.as_ref() {
                app.manage(app_state::AppState::production_namespaced(
                    isolated.paths.clone(),
                    isolated.secret_namespace.clone(),
                )?);
            } else {
                let data_directory = app.path().app_data_dir()?;
                let config_root = app.path().config_dir()?;
                let config_dirs = config::ConfigDirs {
                    repository: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .expect("manifest has parent")
                        .to_path_buf(),
                    roaming_app_data: config_root,
                };
                let config_location = config::locate_config(
                    &arguments,
                    &std::env::vars_os()
                        .filter(|(key, _)| key == "AI_VIRTUAL_ASSISTANT_CONFIG")
                        .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
                        .collect(),
                    &config_dirs,
                    cfg!(debug_assertions),
                )?;
                app.manage(app_state::AppState::production(app_state::AppPaths {
                    logs_directory: data_directory.join("logs"),
                    config_path: config_location.path,
                    data_directory,
                    legacy_search_roots: migrate::legacy_search_roots(
                        &config_dirs,
                        cfg!(debug_assertions),
                    ),
                })?);
            }
            let mut window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()));
            if let Some(isolated) = isolated.as_ref() {
                window = window.data_directory(isolated.webview_data_directory.clone());
            }
            window
                .title("RoleAI")
                .inner_size(1180.0, 760.0)
                .min_inner_size(900.0, 620.0)
                // 设置后 wry 不再追加默认参数，需自行补全；--use-fake-ui-for-media-stream
                // 让 WebView2 自动授予麦克风/摄像头权限（wry 默认不处理该权限请求，页面静默被拒）。
                .additional_browser_args(&webview_browser_args())
                .on_navigation(navigation_is_allowed)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            let assistant_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyA);
            let registered = app.global_shortcut().register(assistant_shortcut).is_ok();
            let _ = app.emit("session:assistant_hotkey_status:v1", registered);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run RoleAI");
}

fn webview_browser_args() -> String {
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --use-fake-ui-for-media-stream",
    );
    if cfg!(debug_assertions) {
        // 开发版开放本机 CDP 端口（仅 127.0.0.1）读取播放诊断；正式包不带。
        args.push_str(" --remote-debugging-port=9223");
        // 开发版可用 WAV 充当麦克风做端到端复现，不需要真人对着麦克风说话。
        if let Ok(wav) = std::env::var("ROLEAI_FAKE_MIC_WAV")
            && !wav.trim().is_empty()
            && !wav.contains('"')
        {
            args.push_str(&format!(
                " --use-fake-device-for-media-stream --use-file-for-fake-audio-capture=\"{wav}\""
            ));
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::navigation_is_allowed;

    #[test]
    fn blocks_remote_navigation() {
        assert!(!navigation_is_allowed(
            &"https://example.com/path".parse().unwrap()
        ));
        assert!(!navigation_is_allowed(
            &"http://127.0.0.1:3000/".parse().unwrap()
        ));
    }

    #[test]
    fn allows_packaged_and_exact_development_origins() {
        assert!(navigation_is_allowed(
            &"tauri://localhost/index.html".parse().unwrap()
        ));
        assert!(navigation_is_allowed(
            &"http://127.0.0.1:1420/".parse().unwrap()
        ));
    }

    #[test]
    fn allows_packaged_windows_origin() {
        assert!(navigation_is_allowed(
            &"http://tauri.localhost/index.html".parse().unwrap()
        ));
    }

    #[test]
    fn blocks_remote_spoof() {
        assert!(!navigation_is_allowed(
            &"http://tauri.localhost.evil.com/index.html"
                .parse()
                .unwrap()
        ));
    }

    #[test]
    fn blocks_https_remote() {
        assert!(!navigation_is_allowed(
            &"https://evil.com/".parse().unwrap()
        ));
    }
}
