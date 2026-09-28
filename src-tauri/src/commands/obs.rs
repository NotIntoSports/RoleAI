//! obs 域命令：OBS 运行时状态、虚拟摄像头控制与 WebSocket 密码管理。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

const OBS_PASSWORD_REF: &str = "obs/websocket-password";

#[tauri::command]
pub fn obs_runtime_status(
    state: State<'_, AppState>,
) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    obs_command(&state, "status")
}

#[tauri::command]
pub fn obs_virtual_camera_start(
    state: State<'_, AppState>,
) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    obs_command(&state, "start")
}

#[tauri::command]
pub fn obs_virtual_camera_stop(
    state: State<'_, AppState>,
) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    obs_command(&state, "stop")
}

fn obs_command(state: &AppState, action: &str) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    let password = match state.secrets.read(OBS_PASSWORD_REF) {
        Ok(value) => value,
        Err(_) => return service_error("SECRET_BACKEND_UNAVAILABLE", "OBS 凭据不可用"),
    };
    let stage_file = state
        .paths
        .data_directory
        .join("livestream")
        .join("stage.html");
    let password = password.as_deref().map(|value| value.as_str());
    let status = match action {
        "start" => {
            let (status, previous) = tauri::async_runtime::block_on(async {
                let previous = crate::obs::current_program_scene(password).await.ok();
                let status = crate::obs::start_virtual_camera(password, &stage_file).await;
                (status, previous)
            });
            if status.virtual_camera_active
                && let Some(previous) = previous.filter(|scene| scene != crate::obs::APP_SCENE_NAME)
                && let Ok(mut slot) = state.obs_previous_scene.lock()
            {
                *slot = Some(previous);
            }
            status
        }
        "stop" => {
            let previous = state
                .obs_previous_scene
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            let mut status =
                tauri::async_runtime::block_on(crate::obs::stop_virtual_camera(password));
            if !status.virtual_camera_active
                && let Some(previous) = previous
            {
                if let Err(code) = tauri::async_runtime::block_on(
                    crate::obs::restore_program_scene(password, &previous),
                ) {
                    status.error_code = Some(code.into());
                } else if let Ok(mut slot) = state.obs_previous_scene.lock() {
                    *slot = None;
                }
            }
            status
        }
        _ => tauri::async_runtime::block_on(crate::obs::ensure_stage(password, &stage_file)),
    };
    CommandResult::Ok { data: status }
}

#[tauri::command]
pub fn obs_password_status(
    state: State<'_, AppState>,
) -> CommandResult<crate::contracts::SecretStatus> {
    match state.secrets.status(OBS_PASSWORD_REF) {
        Ok(data) => CommandResult::Ok { data },
        Err(error) => service_error(error.code(), "OBS 凭据状态不可用"),
    }
}

#[tauri::command]
pub fn obs_password_save(
    state: State<'_, AppState>,
    password: String,
) -> CommandResult<crate::contracts::SecretStatus> {
    if password.len() > 1024 || password.contains(['\r', '\n', '\0']) {
        return service_error("OBS_PASSWORD_INVALID", "OBS 密码无效");
    }
    let result = if password.is_empty() {
        state.secrets.delete(OBS_PASSWORD_REF)
    } else {
        state.secrets.set(OBS_PASSWORD_REF, &password)
    };
    match result {
        Ok(data) => CommandResult::Ok { data },
        Err(error) => service_error(error.code(), "OBS 密码保存失败"),
    }
}
