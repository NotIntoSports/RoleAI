use std::path::Path;

use obws::{
    Client,
    requests::{
        custom::source_settings::{BrowserSource, SOURCE_BROWSER_SOURCE},
        inputs::{Create, SetSettings},
    },
};
use serde::Serialize;
use ts_rs::TS;

pub const APP_SCENE_NAME: &str = "AI Virtual Assistant · Livestream";
pub const APP_BROWSER_SOURCE_NAME: &str = "AI Virtual Assistant · Stage";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsResourcePlan {
    pub create_scene: bool,
    pub create_browser_source: bool,
    pub update_browser_source: bool,
    pub source_name_conflict: bool,
}

pub fn plan_resources(
    existing_scenes: &[String],
    existing_inputs: &[String],
    owned_scene_items: &[String],
) -> ObsResourcePlan {
    let scene_exists = existing_scenes.iter().any(|name| name == APP_SCENE_NAME);
    let source_exists = existing_inputs
        .iter()
        .any(|name| name == APP_BROWSER_SOURCE_NAME);
    let source_is_owned = owned_scene_items
        .iter()
        .any(|name| name == APP_BROWSER_SOURCE_NAME);
    ObsResourcePlan {
        create_scene: !scene_exists,
        create_browser_source: !source_exists,
        update_browser_source: source_exists && source_is_owned,
        source_name_conflict: source_exists && !source_is_owned,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ObsRuntimeStatus {
    pub connected: bool,
    pub scene_ready: bool,
    pub browser_source_ready: bool,
    pub virtual_camera_active: bool,
    pub error_code: Option<String>,
}

impl ObsRuntimeStatus {
    pub fn unavailable(code: &str) -> Self {
        Self {
            connected: false,
            scene_ready: false,
            browser_source_ready: false,
            virtual_camera_active: false,
            error_code: Some(code.into()),
        }
    }
}

pub async fn ensure_stage(password: Option<&str>, stage_file: &Path) -> ObsRuntimeStatus {
    let client = match Client::connect("127.0.0.1", 4455, password).await {
        Ok(client) => client,
        Err(_) => return ObsRuntimeStatus::unavailable("OBS_CONNECT_FAILED"),
    };
    match ensure_stage_with_client(&client, stage_file).await {
        Ok(()) => match client.virtual_cam().status().await {
            Ok(active) => ObsRuntimeStatus {
                connected: true,
                scene_ready: true,
                browser_source_ready: true,
                virtual_camera_active: active,
                error_code: None,
            },
            Err(_) => ObsRuntimeStatus {
                connected: true,
                scene_ready: true,
                browser_source_ready: true,
                virtual_camera_active: false,
                error_code: Some("OBS_VIRTUAL_CAMERA_STATUS_FAILED".into()),
            },
        },
        Err(code) => ObsRuntimeStatus {
            connected: true,
            scene_ready: false,
            browser_source_ready: false,
            virtual_camera_active: false,
            error_code: Some(code.into()),
        },
    }
}

pub async fn start_virtual_camera(password: Option<&str>, stage_file: &Path) -> ObsRuntimeStatus {
    let client = match Client::connect("127.0.0.1", 4455, password).await {
        Ok(client) => client,
        Err(_) => return ObsRuntimeStatus::unavailable("OBS_CONNECT_FAILED"),
    };
    if let Err(code) = ensure_stage_with_client(&client, stage_file).await {
        return ObsRuntimeStatus {
            connected: true,
            scene_ready: false,
            browser_source_ready: false,
            virtual_camera_active: false,
            error_code: Some(code.into()),
        };
    }
    if client
        .scenes()
        .set_current_program_scene(APP_SCENE_NAME)
        .await
        .is_err()
        || client.virtual_cam().start().await.is_err()
    {
        return ObsRuntimeStatus {
            connected: true,
            scene_ready: true,
            browser_source_ready: true,
            virtual_camera_active: false,
            error_code: Some("OBS_VIRTUAL_CAMERA_START_FAILED".into()),
        };
    }
    let active = client.virtual_cam().status().await.unwrap_or(false);
    ObsRuntimeStatus {
        connected: true,
        scene_ready: true,
        browser_source_ready: true,
        virtual_camera_active: active,
        error_code: (!active).then(|| "OBS_VIRTUAL_CAMERA_START_UNVERIFIED".into()),
    }
}

pub async fn stop_virtual_camera(password: Option<&str>) -> ObsRuntimeStatus {
    let client = match Client::connect("127.0.0.1", 4455, password).await {
        Ok(client) => client,
        Err(_) => return ObsRuntimeStatus::unavailable("OBS_CONNECT_FAILED"),
    };
    if client.virtual_cam().stop().await.is_err() {
        return ObsRuntimeStatus {
            connected: true,
            scene_ready: true,
            browser_source_ready: true,
            virtual_camera_active: true,
            error_code: Some("OBS_VIRTUAL_CAMERA_STOP_FAILED".into()),
        };
    }
    let active = client.virtual_cam().status().await.unwrap_or(true);
    ObsRuntimeStatus {
        connected: true,
        scene_ready: true,
        browser_source_ready: true,
        virtual_camera_active: active,
        error_code: active.then(|| "OBS_VIRTUAL_CAMERA_STOP_UNVERIFIED".into()),
    }
}

pub async fn current_program_scene(password: Option<&str>) -> Result<String, &'static str> {
    let client = Client::connect("127.0.0.1", 4455, password)
        .await
        .map_err(|_| "OBS_CONNECT_FAILED")?;
    client
        .scenes()
        .current_program_scene()
        .await
        .map(|scene| scene.id.name)
        .map_err(|_| "OBS_CURRENT_SCENE_FAILED")
}

pub async fn restore_program_scene(
    password: Option<&str>,
    previous_scene: &str,
) -> Result<(), &'static str> {
    let client = Client::connect("127.0.0.1", 4455, password)
        .await
        .map_err(|_| "OBS_CONNECT_FAILED")?;
    let current = client
        .scenes()
        .current_program_scene()
        .await
        .map_err(|_| "OBS_CURRENT_SCENE_FAILED")?;
    let scenes = client
        .scenes()
        .list()
        .await
        .map_err(|_| "OBS_SCENE_LIST_FAILED")?;
    if should_restore_scene(
        &current.id.name,
        previous_scene,
        &scenes
            .scenes
            .iter()
            .map(|scene| scene.id.name.clone())
            .collect::<Vec<_>>(),
    ) {
        client
            .scenes()
            .set_current_program_scene(previous_scene)
            .await
            .map_err(|_| "OBS_SCENE_RESTORE_FAILED")?;
    }
    Ok(())
}

pub fn should_restore_scene(current: &str, previous: &str, scenes: &[String]) -> bool {
    current == APP_SCENE_NAME
        && previous != APP_SCENE_NAME
        && scenes.iter().any(|scene| scene == previous)
}

async fn ensure_stage_with_client(client: &Client, stage_file: &Path) -> Result<(), &'static str> {
    if !stage_file.is_file() {
        return Err("OBS_STAGE_FILE_MISSING");
    }
    let scenes = client
        .scenes()
        .list()
        .await
        .map_err(|_| "OBS_SCENE_LIST_FAILED")?;
    let scene_names = scenes
        .scenes
        .iter()
        .map(|scene| scene.id.name.clone())
        .collect::<Vec<_>>();
    if !scene_names.iter().any(|name| name == APP_SCENE_NAME) {
        client
            .scenes()
            .create(APP_SCENE_NAME)
            .await
            .map_err(|_| "OBS_SCENE_CREATE_FAILED")?;
    }
    let inputs = client
        .inputs()
        .list(None)
        .await
        .map_err(|_| "OBS_INPUT_LIST_FAILED")?;
    let input_names = inputs
        .iter()
        .map(|input| input.id.name.clone())
        .collect::<Vec<_>>();
    let owned_scene_items = client
        .scene_items()
        .list(APP_SCENE_NAME.into())
        .await
        .map_err(|_| "OBS_SCENE_ITEM_LIST_FAILED")?
        .into_iter()
        .map(|item| item.source_name)
        .collect::<Vec<_>>();
    let plan = plan_resources(&scene_names, &input_names, &owned_scene_items);
    if plan.source_name_conflict {
        return Err("OBS_OWNED_SOURCE_NAME_CONFLICT");
    }
    let settings = BrowserSource {
        is_local_file: true,
        local_file: stage_file,
        url: "",
        width: 1920,
        height: 1080,
        fps_custom: true,
        fps: 30,
        reroute_audio: false,
        css: "body{margin:0;overflow:hidden;background:#0b1020}",
        shutdown: false,
        restart_when_active: false,
    };
    if plan.create_browser_source {
        client
            .inputs()
            .create(Create {
                scene: APP_SCENE_NAME.into(),
                input: APP_BROWSER_SOURCE_NAME,
                kind: SOURCE_BROWSER_SOURCE,
                settings: Some(settings),
                enabled: Some(true),
            })
            .await
            .map_err(|_| "OBS_BROWSER_SOURCE_CREATE_FAILED")?;
    } else {
        client
            .inputs()
            .set_settings(SetSettings {
                input: APP_BROWSER_SOURCE_NAME.into(),
                settings: &settings,
                overlay: Some(true),
            })
            .await
            .map_err(|_| "OBS_BROWSER_SOURCE_UPDATE_FAILED")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_plan_is_idempotent_and_only_targets_owned_names() {
        let first = plan_resources(&["用户场景".into()], &["用户摄像头".into()], &[]);
        assert!(first.create_scene);
        assert!(first.create_browser_source);
        assert!(!first.update_browser_source);
        assert!(!first.source_name_conflict);

        let next = plan_resources(
            &["用户场景".into(), APP_SCENE_NAME.into()],
            &["用户摄像头".into(), APP_BROWSER_SOURCE_NAME.into()],
            &[APP_BROWSER_SOURCE_NAME.into()],
        );
        assert!(!next.create_scene);
        assert!(!next.create_browser_source);
        assert!(next.update_browser_source);
        assert!(!next.source_name_conflict);
    }

    #[test]
    fn resource_plan_rejects_a_global_name_collision_outside_the_owned_scene() {
        let plan = plan_resources(
            &["用户场景".into(), APP_SCENE_NAME.into()],
            &[APP_BROWSER_SOURCE_NAME.into()],
            &[],
        );
        assert!(plan.source_name_conflict);
        assert!(!plan.create_browser_source);
        assert!(!plan.update_browser_source);
    }

    #[test]
    fn unavailable_status_does_not_claim_virtual_camera_output() {
        let status = ObsRuntimeStatus::unavailable("OBS_CONNECT_FAILED");
        assert!(!status.connected);
        assert!(!status.virtual_camera_active);
        assert_eq!(status.error_code.as_deref(), Some("OBS_CONNECT_FAILED"));
    }

    #[test]
    fn restores_only_when_the_app_scene_is_still_current() {
        let scenes = vec![APP_SCENE_NAME.into(), "用户场景".into()];
        assert!(should_restore_scene(APP_SCENE_NAME, "用户场景", &scenes));
        assert!(!should_restore_scene(
            "用户刚切换的场景",
            "用户场景",
            &scenes
        ));
        assert!(!should_restore_scene(APP_SCENE_NAME, "已删除场景", &scenes));
    }

    #[test]
    fn virtual_camera_commands_report_stable_code_without_password_when_obs_is_down() {
        // 占住 4455 并立即掐断每个连接：无论本机是否装 OBS，WebSocket 握手必败，
        // 命令必须返回稳定错误码而不是 panic 或把密码带出去。
        let listener = std::net::TcpListener::bind("127.0.0.1:4455").unwrap();
        let stop_janitor = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let janitor_listener = listener.try_clone().unwrap();
        let janitor_flag = std::sync::Arc::clone(&stop_janitor);
        let janitor = std::thread::spawn(move || {
            let _ = janitor_listener.set_nonblocking(true);
            while !janitor_flag.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok((stream, _)) = janitor_listener.accept() {
                    drop(stream);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        let password = "sk-test-obs-password-do-not-leak";
        let start = tauri::async_runtime::block_on(super::start_virtual_camera(
            Some(password),
            std::path::Path::new("stage.html"),
        ));
        assert_eq!(start.error_code.as_deref(), Some("OBS_CONNECT_FAILED"));
        assert!(!start.connected);
        assert!(!start.virtual_camera_active);
        assert!(
            !format!("{start:?}").contains(password),
            "断连状态不得携带密码"
        );

        let stop = tauri::async_runtime::block_on(super::stop_virtual_camera(Some(password)));
        assert_eq!(stop.error_code.as_deref(), Some("OBS_CONNECT_FAILED"));
        assert!(!stop.connected);

        let scene = tauri::async_runtime::block_on(super::current_program_scene(Some(password)));
        assert_eq!(scene.unwrap_err(), "OBS_CONNECT_FAILED");
        stop_janitor.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(listener);
        janitor.join().unwrap();
    }
}
