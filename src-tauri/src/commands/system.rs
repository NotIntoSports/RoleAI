//! system 域命令：foundation 状态、诊断导出、legacy 迁移、目录/网页打开、
//! 会议进程枚举、音频输出枚举与虚拟音频前置条件（安装/状态）。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

#[tauri::command]
pub fn foundation_get_status() -> CommandResult<FoundationStatus> {
    CommandResult::Ok {
        data: FoundationStatus { ready: true },
    }
}

pub fn diagnostics_export_blocking(
    state: State<'_, AppState>,
    destination: String,
) -> CommandResult<DiagnosticsExportResult> {
    diagnostics_export_cmd(&state, destination)
}

fn diagnostics_export_cmd(
    state: &AppState,
    destination: String,
) -> CommandResult<DiagnosticsExportResult> {
    // The renderer is untrusted: a compromised page must not be able to place
    // diagnostic files at arbitrary filesystem locations, so writes are pinned
    // to the application data directory.
    if let Err(error) = ensure_diagnostics_destination(state, &destination) {
        return CommandResult::Err { error };
    }
    let public_config = match state.config.load() {
        Ok(config) => match serde_json::to_value(diagnostic_view(&config)) {
            Ok(value) => value,
            Err(_) => {
                return CommandResult::Err {
                    error: PublicError::new(
                        "DIAGNOSTICS_OPERATION_FAILED",
                        "Diagnostic operation failed",
                        false,
                    ),
                };
            }
        },
        Err(error) => {
            return CommandResult::Err {
                error: PublicError::new(
                    error.code(),
                    "Configuration is unavailable for export",
                    false,
                ),
            };
        }
    };
    let database_status = state
        .database
        .lock()
        .ok()
        .and_then(|database| {
            database
                .as_ref()
                .and_then(|database| database.integrity_check().ok())
        })
        .unwrap_or_else(|| "unavailable".to_owned());
    let service_status = serde_json::json!({ "database": database_status });
    match state.diagnostics.export(
        std::path::Path::new(&destination),
        public_config,
        service_status,
    ) {
        Ok(()) => CommandResult::Ok {
            data: DiagnosticsExportResult { exported: true },
        },
        Err(error) => CommandResult::Err {
            error: PublicError::new(error.code(), error.to_string(), false),
        },
    }
}

fn ensure_diagnostics_destination(state: &AppState, destination: &str) -> Result<(), PublicError> {
    const INVALID: &str = "DIAGNOSTICS_DESTINATION_INVALID";
    let destination = std::path::Path::new(destination);
    if !destination.is_absolute() {
        return Err(PublicError::new(
            INVALID,
            "诊断报告只能导出到应用数据目录",
            false,
        ));
    }
    std::fs::create_dir_all(&state.paths.data_directory).map_err(|_| {
        PublicError::new(
            "DIAGNOSTICS_OPERATION_FAILED",
            "Diagnostic operation failed",
            false,
        )
    })?;
    let data_root = state.paths.data_directory.canonicalize().map_err(|_| {
        PublicError::new(
            "DIAGNOSTICS_OPERATION_FAILED",
            "Diagnostic operation failed",
            false,
        )
    })?;
    let Some(parent) = destination.parent() else {
        return Err(PublicError::new(
            INVALID,
            "诊断报告只能导出到应用数据目录",
            false,
        ));
    };
    let parent = parent
        .canonicalize()
        .map_err(|_| PublicError::new(INVALID, "诊断报告只能导出到应用数据目录", false))?;
    if !parent.starts_with(&data_root) {
        return Err(PublicError::new(
            INVALID,
            "诊断报告只能导出到应用数据目录",
            false,
        ));
    }
    Ok(())
}

pub(super) fn legacy_migration_status_cmd(
    state: &AppState,
) -> CommandResult<LegacyMigrationStatus> {
    let configured = match state.config.load() {
        Ok(config) => crate::migrate::secret_slots_configured(&config),
        Err(_) => false,
    };
    CommandResult::Ok {
        data: crate::migrate::legacy_migration_status(&state.paths.data_directory, configured),
    }
}

pub fn legacy_migration_status_blocking(
    state: State<'_, AppState>,
) -> CommandResult<LegacyMigrationStatus> {
    legacy_migration_status_cmd(&state)
}

fn resolve_legacy_source_path(
    path: &str,
) -> Result<std::path::PathBuf, crate::migrate::MigrateError> {
    let path = std::path::Path::new(path);
    if path.is_relative() {
        return Err(crate::migrate::MigrateError::Operation);
    }
    if std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(crate::migrate::MigrateError::Operation);
    }
    std::fs::canonicalize(path).map_err(|_| crate::migrate::MigrateError::Operation)
}

fn migrate_error<T: ts_rs::TS>(error: crate::migrate::MigrateError) -> CommandResult<T> {
    let message = match error {
        crate::migrate::MigrateError::PayloadInvalid => "旧会话数据无法解析",
        crate::migrate::MigrateError::AlreadyApplied => "目标数据目录已经完成迁移",
        _ => "无法从所选目录导入旧会话",
    };
    service_error(error.code(), message)
}

pub(super) fn legacy_import_source_cmd(
    state: &AppState,
    path: String,
) -> CommandResult<LegacySessionImport> {
    let source = match resolve_legacy_source_path(&path) {
        Ok(path) => path,
        Err(error) => return migrate_error(error),
    };
    // 只借 Arc：导入可能持续数秒（大归档），期间记录/资料/诊断命令照常读库
    // （Database 内部自带连接互斥）。service_guard 由命令包装层持有，
    // 导入期间会话开始/追答等仍被串行化，避免与 sessions 表写入交错。
    let database_slot = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database_slot.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let database = std::sync::Arc::clone(database);
    drop(database_slot);
    crate::migrate::import_legacy_sessions_from_user_path(&source, &database)
        .map_or_else(migrate_error, |data| CommandResult::Ok { data })
}

pub fn legacy_import_source_blocking(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<LegacySessionImport> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    legacy_import_source_cmd(&state, path)
}
#[tauri::command]
pub fn open_app_directory(
    state: State<'_, AppState>,
    kind: String,
) -> CommandResult<FoundationStatus> {
    let directory = match kind.as_str() {
        "config" => state
            .paths
            .config_path
            .parent()
            .map(std::path::Path::to_path_buf),
        "data" => Some(state.paths.data_directory.clone()),
        _ => None,
    };
    let Some(directory) = directory else {
        return CommandResult::Err {
            error: PublicError::new(
                "APP_DIRECTORY_INVALID",
                "Unsupported application directory",
                false,
            ),
        };
    };
    match open_directory(&directory) {
        Ok(()) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(()) => CommandResult::Err {
            error: PublicError::new(
                "APP_DIRECTORY_OPEN_FAILED",
                "Application directory could not be opened",
                false,
            ),
        },
    }
}

#[cfg(windows)]
fn open_directory(path: &std::path::Path) -> Result<(), ()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let operation = "open".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize > 32 {
        Ok(())
    } else {
        Err(())
    }
}

#[tauri::command]
pub fn open_web_source(url: String) -> CommandResult<FoundationStatus> {
    let parsed = match reqwest::Url::parse(&url) {
        Ok(url)
            if matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            url
        }
        _ => return service_error("WEB_SOURCE_INVALID", "来源地址无效"),
    };
    // ShellExecute opens only a validated web URL; no shell command interpolation.
    match open_directory(std::path::Path::new(parsed.as_str())) {
        Ok(()) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(()) => service_error("WEB_SOURCE_OPEN_FAILED", "无法打开系统浏览器"),
    }
}

#[cfg(not(windows))]
fn open_directory(_: &std::path::Path) -> Result<(), ()> {
    Err(())
}
blocking_command!(diagnostics_export, diagnostics_export_blocking(destination: String) -> DiagnosticsExportResult);
blocking_command!(legacy_migration_status, legacy_migration_status_blocking() -> LegacyMigrationStatus);
blocking_command!(legacy_import_source, legacy_import_source_blocking(path: String) -> LegacySessionImport);

/// 麦克风路径智能复核：把前端探针证据交激活线路的大模型诊断。
/// 只产出诊断文本与受白名单约束的建议设备 id，不改绑定、不碰系统状态；
/// 任何失败都返回稳定错误码，前端据此降级为确定性选型。
pub fn session_audio_path_review_blocking(
    state: State<'_, AppState>,
    evidence: AudioPathEvidence,
) -> CommandResult<AudioPathReview> {
    use crate::services::audio_diagnosis::{self, AudioDiagnosisError};
    let degrade = |error: AudioDiagnosisError| service_error(error.code(), error.public_message());
    if let Err(error) = audio_diagnosis::validate_evidence(&evidence) {
        return degrade(error);
    }
    let config = match super::sessions::load_session_config(&state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    let route = match active_voice_route(&config) {
        Some(route) => route,
        None => return degrade(AudioDiagnosisError::EndpointMissing),
    };
    let Some(provider_id) = route.llm_provider_id.as_deref().filter(|id| !id.is_empty()) else {
        return degrade(AudioDiagnosisError::EndpointMissing);
    };
    let Some(model_id) = route.llm_model_id.as_deref().filter(|id| !id.is_empty()) else {
        return degrade(AudioDiagnosisError::EndpointMissing);
    };
    let endpoint = match config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        })
        .filter(|endpoint| !endpoint.base_url.is_empty())
    {
        Some(endpoint) => endpoint,
        None => return degrade(AudioDiagnosisError::EndpointMissing),
    };
    let credential = match read_provider_secret(&state, &config, Some(provider_id)) {
        Ok(credential) => credential,
        Err(error) => return CommandResult::Err { error },
    };
    let client = match OpenAiCompatibleCascade::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "Cascade client is unavailable"),
    };
    match audio_diagnosis::review_audio_path(
        &client,
        &endpoint,
        credential.as_deref().map(String::as_str),
        model_id,
        &evidence,
    ) {
        Ok(review) => CommandResult::Ok { data: review },
        Err(error) => degrade(error),
    }
}
blocking_command!(session_audio_path_review, session_audio_path_review_blocking(evidence: AudioPathEvidence) -> AudioPathReview);

pub fn meeting_process_list_blocking(
    _state: State<'_, AppState>,
) -> CommandResult<Vec<crate::processes::MeetingProcess>> {
    match crate::processes::list_meeting_processes(&crate::processes::PowerShellProcessEnumerator) {
        Ok(data) => CommandResult::Ok { data },
        Err(crate::processes::ProcessError::PlatformUnsupported) => service_error(
            crate::processes::ProcessError::PlatformUnsupported.code(),
            "当前平台不支持会议进程采集",
        ),
        Err(_) => service_error(
            "MEETING_PROCESS_ENUM_FAILED",
            "无法读取会议进程，请确认会议软件已打开",
        ),
    }
}
blocking_command!(meeting_process_list, meeting_process_list_blocking() -> Vec<crate::processes::MeetingProcess>);

pub fn audio_output_list_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    _state: State<'_, AppState>,
) -> CommandResult<Vec<crate::audio::playback::AudioOutputDevice>> {
    if !cfg!(windows) {
        // AudioBridge 是 Windows 专属 sidecar；非 Windows 返回稳定错误码，
        // 前端据此隐藏回环音频设备入口。
        return service_error("PLATFORM_UNSUPPORTED", "当前平台不支持本机回环音频采集");
    }
    let bridge = if cfg!(debug_assertions) {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../native/AudioBridge/publish/AudioBridge.exe")
    } else {
        match app.path().resource_dir() {
            Ok(root) => root.join("audio-bridge/AudioBridge.exe"),
            Err(_) => return service_error("SESSION_SIDECAR_MISSING", "无法定位音频组件"),
        }
    };
    match crate::audio::playback::list_outputs(&bridge) {
        Ok(data) => CommandResult::Ok { data },
        Err(code) => service_error(
            code,
            "无法读取音频输出设备，请确认 AudioBridge 已安装、音频设备已连接",
        ),
    }
}
blocking_command!(with_events audio_output_list, audio_output_list_blocking() -> Vec<crate::audio::playback::AudioOutputDevice>);

pub(super) fn audio_bridge_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<std::path::PathBuf, &'static str> {
    if !cfg!(windows) {
        return Err("PLATFORM_UNSUPPORTED");
    }
    if cfg!(debug_assertions) {
        Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../native/AudioBridge/publish/AudioBridge.exe"))
    } else {
        app.path()
            .resource_dir()
            .map(|root| root.join("audio-bridge/AudioBridge.exe"))
            .map_err(|_| "SESSION_SIDECAR_MISSING")
    }
}

fn prerequisite_script_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
) -> Result<std::path::PathBuf, &'static str> {
    if !matches!(name, "fetch-prerequisites.ps1" | "install-prerequisite.ps1") {
        return Err("PREREQUISITE_RESOURCE_MISSING");
    }
    if cfg!(debug_assertions) {
        Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts")
            .join(name))
    } else {
        app.path()
            .resource_dir()
            .map(|root| root.join("prerequisite-scripts").join(name))
            .map_err(|_| "PREREQUISITE_RESOURCE_MISSING")
    }
}

static AUDIO_PREPARATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn audio_preparation_failure<T: ts_rs::TS>(code: &'static str) -> CommandResult<T> {
    service_error(code, crate::prerequisites::preparation_error_message(code))
}

fn persist_preparation_diagnostic(
    state: &AppState,
    diagnostic: &crate::prerequisites::PreparationDiagnostic,
) -> bool {
    let directory = state.paths.data_directory.join("prerequisites");
    if std::fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let record = directory.join("preparation-result.json");
    serde_json::to_vec(diagnostic)
        .ok()
        .and_then(|bytes| std::fs::write(record, bytes).ok())
        .is_some()
}

fn attach_preparation_diagnostic(
    state: &AppState,
    phase: String,
    exit_code: Option<i32>,
    result: CommandResult<crate::prerequisites::VirtualAudioPreparation>,
) -> CommandResult<crate::prerequisites::VirtualAudioPreparation> {
    let code = match &result {
        CommandResult::Err { error } => Some(error.code.clone()),
        CommandResult::Ok { data } => data
            .diagnostic
            .as_ref()
            .and_then(|item| item.error_code.clone()),
    };
    let retry_allowed = !matches!(
        code.as_deref(),
        Some("PREREQUISITE_TIMEOUT" | "PREREQUISITE_INSTALL_BUSY")
    );
    let diagnostic = crate::prerequisites::PreparationDiagnostic {
        phase: phase.clone(),
        retry_allowed,
        error_code: code.clone(),
        exit_code,
    };
    let persist_ok = persist_preparation_diagnostic(state, &diagnostic);
    match result {
        CommandResult::Ok { mut data } => {
            data.diagnostic = Some(diagnostic);
            if !persist_ok {
                data.detail = format!("{}。诊断记录未能写入。", data.detail);
            }
            CommandResult::Ok { data }
        }
        CommandResult::Err { mut error } => {
            error.message = format!(
                "{}（阶段：{}）",
                error.message,
                crate::prerequisites::preparation_phase_label(&phase)
            );
            if !persist_ok {
                error.message.push_str("。诊断记录未能写入。");
            }
            CommandResult::Err { error }
        }
    }
}

fn audio_preparation_state(
    state: &str,
    detail: &str,
) -> crate::prerequisites::VirtualAudioPreparation {
    let mut result = crate::prerequisites::VirtualAudioPreparation::from_devices(&[]);
    result.state = state.into();
    result.detail = detail.into();
    result
}

pub fn virtual_audio_status_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<crate::prerequisites::VirtualAudioPreparation> {
    let Ok(_guard) = AUDIO_PREPARATION_LOCK.try_lock() else {
        return CommandResult::Ok {
            data: audio_preparation_state("installing", "安装任务正在运行，请等待完成。"),
        };
    };
    let bridge = match audio_bridge_path(&app) {
        Ok(path) => path,
        Err(code) => return service_error(code, "缺少 AudioBridge 音频组件"),
    };
    match crate::prerequisites::enumerate_audio_devices(&bridge) {
        Ok(devices) => {
            let mut status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
            if !status.installed
                && let Ok(script) = prerequisite_script_path(&app, "install-prerequisite.ps1")
            {
                let directory = state
                    .paths
                    .data_directory
                    .join("prerequisites")
                    .to_string_lossy()
                    .into_owned();
                match crate::prerequisites::run_script_bounded(
                    &script,
                    &[
                        "-Component",
                        "virtual-audio",
                        "-ResourcesDirectory",
                        &directory,
                        "-ProbeOnly",
                    ],
                    std::time::Duration::from_secs(15),
                ) {
                    Ok(probe) if probe["driverStore"] == true && status.state == "missing" => {
                        status = audio_preparation_state(
                            "driver_present",
                            "驱动已在 Windows 中但端点尚未就绪，请检查设备状态；不会重复安装。必要时请自行重启后重新检测。",
                        );
                    }
                    Err("PREREQUISITE_INSTALL_BUSY") => {
                        status = audio_preparation_state(
                            "installing",
                            "提权安装任务仍在运行，请完成授权或等待安装结束。",
                        )
                    }
                    Err(code) => return audio_preparation_failure(code),
                    _ => {}
                }
            }
            CommandResult::Ok { data: status }
        }
        Err(code) => service_error(code, "无法检测本机音频设备"),
    }
}
blocking_command!(with_events virtual_audio_status, virtual_audio_status_blocking() -> crate::prerequisites::VirtualAudioPreparation);

pub fn virtual_audio_install_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<crate::prerequisites::VirtualAudioPreparation> {
    let Ok(_guard) = AUDIO_PREPARATION_LOCK.try_lock() else {
        return attach_preparation_diagnostic(
            &state,
            "checking".into(),
            None,
            audio_preparation_failure("PREREQUISITE_INSTALL_BUSY"),
        );
    };
    let current_phase = std::cell::RefCell::new("checking".to_string());
    let exit_code = std::cell::Cell::new(None);
    let phase = |phase: &str| {
        *current_phase.borrow_mut() = phase.into();
        let _ = app.emit("virtual_audio:preparation:v1", phase);
    };
    let result = (|| {
        phase("checking");
        let bridge = match audio_bridge_path(&app) {
            Ok(path) => path,
            Err(code) => return service_error(code, "缺少 AudioBridge 音频组件"),
        };
        match crate::prerequisites::enumerate_audio_devices(&bridge) {
            Ok(devices) => {
                let status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
                if status.installed || status.state != "missing" {
                    return CommandResult::Ok { data: status };
                }
            }
            Err(code) => return audio_preparation_failure(code),
        }
        let managed = state.paths.data_directory.join("prerequisites");
        if std::fs::create_dir_all(&managed).is_err() {
            return service_error("PREREQUISITE_DIRECTORY_FAILED", "无法创建托管安装目录");
        }
        let fetch = match prerequisite_script_path(&app, "fetch-prerequisites.ps1") {
            Ok(path) => path,
            Err(code) => return service_error(code, "缺少虚拟声卡准备脚本"),
        };
        let install = match prerequisite_script_path(&app, "install-prerequisite.ps1") {
            Ok(path) => path,
            Err(code) => return service_error(code, "缺少虚拟声卡安装脚本"),
        };
        let destination = managed.to_string_lossy().to_string();
        // Preflight the worker lock and existing driver before downloading or retrying.
        match crate::prerequisites::run_script_bounded(
            &install,
            &[
                "-Component",
                "virtual-audio",
                "-ResourcesDirectory",
                &destination,
                "-ProbeOnly",
            ],
            std::time::Duration::from_secs(15),
        ) {
            Ok(probe) if probe["driverStore"] == true => {
                return CommandResult::Ok {
                    data: audio_preparation_state(
                        "driver_present",
                        "驱动已经存在，但端点尚未就绪；不会重复安装。请重新检测设备状态。",
                    ),
                };
            }
            Err(code) => return audio_preparation_failure(code),
            _ => {}
        }
        phase("verifying");
        if let Err(code) = crate::prerequisites::run_preparation_script(
            &fetch,
            &["-Component", "virtual-audio", "-Destination", &destination],
            std::time::Duration::from_secs(300),
            &mut |event| match event {
                crate::prerequisites::PreparationEvent::Phase(value) => phase(&value),
                crate::prerequisites::PreparationEvent::Exit(value) => exit_code.set(value),
            },
        ) {
            return audio_preparation_failure(code);
        }
        phase("authorizing");
        exit_code.set(None);
        let install_result = match crate::prerequisites::run_preparation_script(
            &install,
            &[
                "-Component",
                "virtual-audio",
                "-ResourcesDirectory",
                &destination,
            ],
            std::time::Duration::from_secs(600),
            &mut |event| match event {
                crate::prerequisites::PreparationEvent::Phase(value) => phase(&value),
                crate::prerequisites::PreparationEvent::Exit(value) => exit_code.set(value),
            },
        ) {
            Ok(Some(result)) => result,
            Ok(None) => return audio_preparation_failure("PREREQUISITE_RESULT_INVALID"),
            Err(code) => return audio_preparation_failure(code),
        };
        phase("rechecking");
        if let Ok(devices) = crate::prerequisites::enumerate_audio_devices(&bridge) {
            let status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
            if status.installed {
                return CommandResult::Ok { data: status };
            }
        }
        let reboot_required = install_result["rebootRequired"].as_bool().unwrap_or(false);
        CommandResult::Ok {
            data: crate::prerequisites::VirtualAudioPreparation {
                state: if reboot_required {
                    "reboot_required"
                } else {
                    "failed"
                }
                .into(),
                installed: false,
                reboot_required,
                detail: if reboot_required {
                    "驱动已安装，需要重启 Windows 后继续"
                } else {
                    "安装完成但未检测到虚拟声卡端点"
                }
                .into(),
                render_endpoint_id: None,
                capture_endpoint_id: None,
                diagnostic: None,
            },
        }
    })();
    attach_preparation_diagnostic(&state, current_phase.into_inner(), exit_code.get(), result)
}
blocking_command!(with_events virtual_audio_install, virtual_audio_install_blocking() -> crate::prerequisites::VirtualAudioPreparation);

pub fn diagnostics_latency_summary_blocking(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> CommandResult<DiagnosticsLatencySummary> {
    let limit = limit.unwrap_or(20).clamp(1, 100) as usize;
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable"),
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let store = crate::sessions::store::SessionStore::new(database);
    let sessions = match store.list() {
        Ok(sessions) => sessions,
        Err(_) => return service_error("DIAGNOSTICS_OPERATION_FAILED", "无法读取会话列表"),
    };
    let route_label = |route_id: &str| -> String {
        state
            .config
            .load()
            .ok()
            .and_then(|config| {
                config
                    .speech
                    .voice_routes
                    .iter()
                    .find(|route| route.id == route_id)
                    .map(|route| route.name.clone())
            })
            .unwrap_or_else(|| route_id.to_owned())
    };
    // 更新时间倒序（store.list 语义）扫描会话；按线路 + 模式分组，只统计
    // 最近 TURN_SAMPLE_CAP 轮（默认 200）：会话内事件按时间升序，从最新往回取。
    let mut groups: std::collections::BTreeMap<(String, String), Vec<(String, serde_json::Value)>> =
        std::collections::BTreeMap::new();
    let mut sessions_scanned: u32 = 0;
    let mut turns_collected: usize = 0;
    let mut recent_turns: Vec<crate::contracts::TurnLatencySample> = Vec::new();
    'sessions: for session in sessions.iter().take(limit) {
        let events = match store.list_events(&session.id) {
            Ok(events) => events,
            Err(_) => return service_error("DIAGNOSTICS_OPERATION_FAILED", "无法读取会话事件"),
        };
        let turn_events: Vec<(&str, serde_json::Value)> = events
            .iter()
            .filter(|event| event.kind == "turn_meta")
            .filter_map(|event| {
                serde_json::from_str(&event.payload)
                    .ok()
                    .map(|meta| (event.created_at.as_str(), meta))
            })
            .collect();
        if !turn_events.is_empty() {
            sessions_scanned += 1;
        }
        for (created_at, meta) in turn_events.iter().rev() {
            if turns_collected >= TURN_SAMPLE_CAP {
                break 'sessions;
            }
            let mode = meta
                .get("latencyMode")
                .and_then(|value| value.as_str())
                .unwrap_or("realtime")
                .to_owned();
            groups
                .entry((session.voice_route_id.clone(), mode.clone()))
                .or_default()
                .push((session.id.clone(), meta.clone()));
            turns_collected += 1;
            if recent_turns.len() < RECENT_TURN_SAMPLE_CAP {
                recent_turns.push(crate::contracts::TurnLatencySample {
                    route_id: session.voice_route_id.clone(),
                    mode: mode.clone(),
                    total_ms: crate::sessions::latency_export::turn_total_latency_ms(meta),
                    created_at: (*created_at).to_owned(),
                });
            }
        }
    }
    let routes = groups
        .into_iter()
        .map(|((route_id, mode), metas)| {
            summarize_route(&route_id, &mode, &route_label(&route_id), &metas)
        })
        .collect();
    CommandResult::Ok {
        data: DiagnosticsLatencySummary {
            sessions_scanned,
            routes,
            recent_turns,
        },
    }
}

/// 性能面板折线保留的最近轮数。
const RECENT_TURN_SAMPLE_CAP: usize = 50;

/// 分阶段汇总覆盖的时间线字段（固定顺序 = 时间线字段声明序）。
const TIMELINE_STAGE_FIELDS: [&str; 13] = [
    "speechStartedMs",
    "speechStoppedMs",
    "transcriptDoneMs",
    "responseCreatedMs",
    "asrDoneMs",
    "retrievalDoneMs",
    "llmFirstTokenMs",
    "llmDoneMs",
    "firstAudioMs",
    "ttsDoneMs",
    "responseDoneMs",
    "playbackStartedMs",
    "playbackDoneMs",
];

/// 汇总统计覆盖的最近轮数上限（跨会话全局计）。
const TURN_SAMPLE_CAP: usize = 200;

/// 单线路单模式汇总：样本数、首响最近秩百分位、分阶段百分位、
/// 每会话最大入口丢帧之和。`ingressDropped` 在写入侧是会话内累计值，
/// 逐轮求和会重复计数，因此按会话取最大值后再按线路求和。
fn summarize_route(
    route_id: &str,
    mode: &str,
    route_label: &str,
    metas: &[(String, serde_json::Value)],
) -> crate::contracts::RouteLatencySummary {
    let mut latencies: Vec<f64> = metas
        .iter()
        .filter_map(|(_, meta)| {
            meta.get("latencyMsFirstAudio")
                .and_then(|value| value.as_f64())
        })
        .collect();
    latencies.sort_by(|a, b| a.total_cmp(b));
    let stages = TIMELINE_STAGE_FIELDS
        .iter()
        .filter_map(|stage| {
            let mut values: Vec<f64> = metas
                .iter()
                .filter_map(|(_, meta)| {
                    meta.get("timeline")
                        .and_then(|timeline| timeline.get(stage))
                        .and_then(|value| value.as_f64())
                })
                .collect();
            if values.is_empty() {
                return None;
            }
            values.sort_by(|a, b| a.total_cmp(b));
            Some(crate::contracts::StageLatencySummary {
                stage: (*stage).to_owned(),
                samples: values.len() as u32,
                p50_ms: nearest_rank_percentile(&values, 50.0),
                p95_ms: nearest_rank_percentile(&values, 95.0),
            })
        })
        .collect();
    let mut dropped_by_session: std::collections::HashMap<&str, u64> =
        std::collections::HashMap::new();
    for (session_id, meta) in metas {
        let dropped = meta
            .get("ingressDropped")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let slot = dropped_by_session.entry(session_id.as_str()).or_insert(0);
        if dropped > *slot {
            *slot = dropped;
        }
    }
    crate::contracts::RouteLatencySummary {
        route_id: route_id.to_owned(),
        route_label: route_label.to_owned(),
        mode: mode.to_owned(),
        samples: metas.len() as u32,
        p50_ms: nearest_rank_percentile(&latencies, 50.0),
        p95_ms: nearest_rank_percentile(&latencies, 95.0),
        stages,
        ingress_dropped_total: dropped_by_session.values().sum::<u64>() as u32,
    }
}

/// 最近秩百分位：升序样本取 ceil(p/100·n)-1 下标；空样本返回 None。
fn nearest_rank_percentile(sorted: &[f64], percentile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((percentile / 100.0) * sorted.len() as f64).ceil() as usize;
    let rank = rank.clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

blocking_command!(diagnostics_latency_summary, diagnostics_latency_summary_blocking(limit: Option<u32>) -> DiagnosticsLatencySummary);

#[cfg(test)]
mod latency_summary_tests {
    use super::*;

    #[test]
    fn percentile_is_none_for_empty_samples() {
        assert_eq!(nearest_rank_percentile(&[], 50.0), None);
    }

    #[test]
    fn percentile_of_single_sample_is_that_sample() {
        assert_eq!(nearest_rank_percentile(&[42.0], 50.0), Some(42.0));
        assert_eq!(nearest_rank_percentile(&[42.0], 95.0), Some(42.0));
    }

    #[test]
    fn percentile_uses_nearest_rank_for_even_counts() {
        // n=4：p50 → ceil(0.5·4)=2 → 第 2 个；p95 → ceil(0.95·4)=4 → 第 4 个。
        let sorted = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(nearest_rank_percentile(&sorted, 50.0), Some(20.0));
        assert_eq!(nearest_rank_percentile(&sorted, 95.0), Some(40.0));
    }

    #[test]
    fn summary_skips_null_latencies_for_percentiles_but_counts_samples() {
        let metas = vec![
            (
                "s1".to_owned(),
                serde_json::json!({"latencyMsFirstAudio":100,"ingressDropped":5}),
            ),
            (
                "s2".to_owned(),
                serde_json::json!({"latencyMsFirstAudio":200,"ingressDropped":9}),
            ),
            (
                "s3".to_owned(),
                serde_json::json!({"latencyMsFirstAudio":null,"ingressDropped":2}),
            ),
        ];
        let summary = summarize_route("route-1", "realtime", "线路一", &metas);
        assert_eq!(summary.mode, "realtime");
        assert_eq!(summary.samples, 3);
        // n=2 非空样本 [100,200]：p50 → 第 1 个? ceil(0.5·2)=1 → 100；p95 → ceil(0.95·2)=2 → 200。
        assert_eq!(summary.p50_ms, Some(100.0));
        assert_eq!(summary.p95_ms, Some(200.0));
        // 会话 t1..t3 互不相同，各取最大后求和。
        assert_eq!(summary.ingress_dropped_total, 16);
    }

    #[test]
    fn summary_all_null_latencies_yield_null_percentiles() {
        let metas = vec![
            (
                "s1".to_owned(),
                serde_json::json!({"latencyMsFirstAudio":null}),
            ),
            (
                "s1".to_owned(),
                serde_json::json!({"latencyMsFirstAudio":null,"ingressDropped":7}),
            ),
        ];
        let summary = summarize_route("route-1", "cascade", "线路一", &metas);
        assert_eq!(summary.mode, "cascade");
        assert_eq!(summary.samples, 2);
        assert_eq!(summary.p50_ms, None);
        assert_eq!(summary.p95_ms, None);
        // 同会话累计值取最大，不重复求和。
        assert_eq!(summary.ingress_dropped_total, 7);
    }

    #[test]
    fn missing_route_falls_back_to_id_as_label() {
        let metas = vec![(
            "s1".to_owned(),
            serde_json::json!({"latencyMsFirstAudio":50}),
        )];
        let summary = summarize_route("route-x", "realtime", "route-x", &metas);
        assert_eq!(summary.route_label, "route-x");
        assert_eq!(summary.p50_ms, Some(50.0));
    }

    #[test]
    fn stage_summary_absent_for_zero_samples_and_exact_for_one() {
        // 0 个样本：阶段不出现在列表；1 个样本：p50 = p95 = 该样本。
        let metas = vec![(
            "s1".to_owned(),
            serde_json::json!({"timeline": {"asrDoneMs": 42}}),
        )];
        let summary = summarize_route("route-1", "cascade", "线路一", &metas);
        assert!(
            summary
                .stages
                .iter()
                .all(|stage| stage.stage != "llmDoneMs")
        );
        let asr = summary
            .stages
            .iter()
            .find(|stage| stage.stage == "asrDoneMs")
            .expect("asr stage present");
        assert_eq!(asr.samples, 1);
        assert_eq!(asr.p50_ms, Some(42.0));
        assert_eq!(asr.p95_ms, Some(42.0));
    }

    #[test]
    fn stage_summary_nearest_rank_for_two_samples_and_skips_nulls() {
        // 2 个样本 [100,200]：p50 → ceil(0.5·2)=1 → 100；p95 → ceil(0.95·2)=2 → 200。
        // null 样本不计入样本数。
        let metas = vec![
            (
                "s1".to_owned(),
                serde_json::json!({"timeline": {"ttsDoneMs": 100}}),
            ),
            (
                "s2".to_owned(),
                serde_json::json!({"timeline": {"ttsDoneMs": 200}}),
            ),
            (
                "s3".to_owned(),
                serde_json::json!({"timeline": {"ttsDoneMs": null}}),
            ),
        ];
        let summary = summarize_route("route-1", "cascade", "线路一", &metas);
        let tts = summary
            .stages
            .iter()
            .find(|stage| stage.stage == "ttsDoneMs")
            .expect("tts stage present");
        assert_eq!(tts.samples, 2);
        assert_eq!(tts.p50_ms, Some(100.0));
        assert_eq!(tts.p95_ms, Some(200.0));
    }

    #[test]
    fn stages_keep_declaration_order() {
        let metas = vec![(
            "s1".to_owned(),
            serde_json::json!({"timeline": {"ttsDoneMs": 30, "asrDoneMs": 10, "llmDoneMs": 20}}),
        )];
        let summary = summarize_route("route-1", "cascade", "线路一", &metas);
        let stages: Vec<&str> = summary
            .stages
            .iter()
            .map(|stage| stage.stage.as_str())
            .collect();
        assert_eq!(stages, ["asrDoneMs", "llmDoneMs", "ttsDoneMs"]);
    }

    #[test]
    fn turn_total_prefers_first_audio_and_falls_back_to_timeline_span() {
        use crate::sessions::latency_export::turn_total_latency_ms;
        assert_eq!(
            turn_total_latency_ms(&serde_json::json!({"latencyMsFirstAudio": 320})),
            Some(320.0)
        );
        assert_eq!(
            turn_total_latency_ms(&serde_json::json!({
                "timeline": {"asrDoneMs": 50, "llmFirstTokenMs": 120, "ttsDoneMs": 480}
            })),
            Some(430.0)
        );
        assert_eq!(
            turn_total_latency_ms(&serde_json::json!({"timeline": {}})),
            None
        );
        assert_eq!(turn_total_latency_ms(&serde_json::json!({})), None);
    }
}
