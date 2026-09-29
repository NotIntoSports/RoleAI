//! OBS 集成：paths 定位本机 OBS 安装与虚拟摄像头，control 经 obs-websocket
//! 控制场景切换、虚拟摄像头启停，并维护 `ObsRuntimeStatus` 状态上报。

pub mod control;
pub mod paths;

pub use control::{
    APP_SCENE_NAME, ObsRuntimeStatus, current_program_scene, ensure_stage, restore_program_scene,
    start_virtual_camera, stop_virtual_camera,
};

pub use paths::{
    OBS_PACKAGED_VERSION, PathError, PathRoots, ResolvedPaths, owned_path, resolve_owned_paths,
};
