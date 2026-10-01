//! Status-only prerequisite probes. Does not install, elevate, or spawn OBS.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::obs::ResolvedPaths;

pub const OBS_VIRTUAL_CAMERA_CLSID: &str = "{A3FCE0F5-3493-419F-958A-ABA1250EC20B}";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct PrerequisiteStatus {
    pub obs_bundled: bool,
    pub virtual_camera_registered: bool,
    pub virtual_audio_installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LocalAudioDevice {
    pub id: String,
    pub name: String,
    pub flow: String,
    #[serde(default = "active_device_state")]
    pub state: String,
}

fn active_device_state() -> String {
    "Active".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct VirtualAudioPreparation {
    pub state: String,
    pub installed: bool,
    pub reboot_required: bool,
    pub detail: String,
    pub render_endpoint_id: Option<String>,
    pub capture_endpoint_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub diagnostic: Option<PreparationDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PreparationDiagnostic {
    pub phase: String,
    pub error_code: Option<String>,
    pub exit_code: Option<i32>,
    pub retry_allowed: bool,
}

pub enum PreparationEvent {
    Phase(String),
    Exit(Option<i32>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRoutingChange {
    pub bridge: PathBuf,
    /// 会话前“默认通信”角色的采集端点；人工接管模式用它把物理麦送回虚拟线路。
    pub previous_id: String,
    pub cable_id: String,
    pub changed: bool,
    /// 会话前“默认设备”(console) 角色；旧版本记录没有这两个字段，恢复时按 None 跳过。
    #[serde(default)]
    pub previous_console_id: Option<String>,
    /// 会话前“多媒体”(multimedia) 角色。
    #[serde(default)]
    pub previous_multimedia_id: Option<String>,
}

fn audio_routing_record_path(data_directory: &Path) -> PathBuf {
    data_directory
        .join("prerequisites")
        .join("audio-routing.json")
}

pub fn persist_audio_routing(
    data_directory: &Path,
    change: &AudioRoutingChange,
) -> Result<(), &'static str> {
    if !change.changed {
        clear_persisted_audio_routing(data_directory);
        return Ok(());
    }
    let directory = data_directory.join("prerequisites");
    std::fs::create_dir_all(&directory).map_err(|_| "AUDIO_ROUTING_PERSIST_FAILED")?;
    let bytes = serde_json::to_vec(change).map_err(|_| "AUDIO_ROUTING_PERSIST_FAILED")?;
    std::fs::write(audio_routing_record_path(data_directory), bytes)
        .map_err(|_| "AUDIO_ROUTING_PERSIST_FAILED")
}

pub fn load_persisted_audio_routing(data_directory: &Path) -> Option<AudioRoutingChange> {
    let bytes = std::fs::read(audio_routing_record_path(data_directory)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn clear_persisted_audio_routing(data_directory: &Path) {
    let _ = std::fs::remove_file(audio_routing_record_path(data_directory));
}

pub fn recover_persisted_audio_routing(data_directory: &Path) -> Option<AudioRoutingChange> {
    let change = load_persisted_audio_routing(data_directory)?;
    match restore_default_capture_mic(&change) {
        Ok(()) => {
            clear_persisted_audio_routing(data_directory);
            None
        }
        Err(_) => Some(change),
    }
}

pub fn configure_default_capture_mic(bridge: &Path) -> Result<AudioRoutingChange, &'static str> {
    let status = VirtualAudioPreparation::from_devices(&enumerate_audio_devices(bridge)?);
    let capture_id = status
        .capture_endpoint_id
        .filter(|_| status.installed)
        .ok_or("CABLE_OUTPUT_NOT_FOUND")?;
    let bytes = run_bounded(
        Command::new(bridge)
            .arg("--set-default-capture-mic")
            .arg(&capture_id),
        Duration::from_secs(15),
        16 * 1024,
    )?;
    parse_capture_mic_change(bridge, &bytes)
}

/// 解析 AudioBridge `--set-default-capture-mic` 的 JSON 输出。
fn parse_capture_mic_change(
    bridge: &Path,
    bytes: &[u8],
) -> Result<AudioRoutingChange, &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "AUDIO_ROUTING_RESULT_INVALID")?;
    let cable_id = value["cableId"]
        .as_str()
        .ok_or("AUDIO_ROUTING_RESULT_INVALID")?
        .to_owned();
    let role_previous_id = |role: &str| -> Result<String, &'static str> {
        value["roles"]
            .as_array()
            .and_then(|roles| {
                roles
                    .iter()
                    .find(|entry| entry["role"].as_str() == Some(role))
                    .and_then(|entry| entry["previousId"].as_str())
            })
            .map(str::to_owned)
            .ok_or("AUDIO_ROUTING_RESULT_INVALID")
    };
    Ok(AudioRoutingChange {
        bridge: bridge.to_path_buf(),
        previous_id: role_previous_id("Communications")?,
        previous_console_id: Some(role_previous_id("Console")?),
        previous_multimedia_id: Some(role_previous_id("Multimedia")?),
        cable_id,
        changed: value["changed"].as_bool().unwrap_or(false),
    })
}

pub fn restore_default_capture_mic(change: &AudioRoutingChange) -> Result<(), &'static str> {
    if !change.changed {
        return Ok(());
    }
    run_bounded(
        Command::new(&change.bridge)
            .arg("--restore-default-capture-mic")
            .arg(&change.cable_id)
            .arg(change.previous_console_id.as_deref().unwrap_or(""))
            .arg(change.previous_multimedia_id.as_deref().unwrap_or(""))
            .arg(&change.previous_id),
        Duration::from_secs(15),
        1024,
    )
    .map(|_| ())
}

impl VirtualAudioPreparation {
    pub fn from_devices(devices: &[LocalAudioDevice]) -> Self {
        let render = devices
            .iter()
            .find(|d| d.flow == "render" && is_cable_input(&d.name) && d.state == "Active");
        let capture = devices
            .iter()
            .find(|d| d.flow == "capture" && is_cable_output(&d.name) && d.state == "Active");
        let installed = render.is_some() && capture.is_some();
        let disabled = devices.iter().any(|d| {
            (is_cable_input(&d.name) || is_cable_output(&d.name)) && d.state == "Disabled"
        });
        let partial = devices
            .iter()
            .any(|d| is_cable_input(&d.name) || is_cable_output(&d.name));
        Self {
            state: if installed {
                "ready"
            } else if disabled {
                "disabled"
            } else if partial {
                "incomplete"
            } else {
                "missing"
            }
            .into(),
            installed,
            reboot_required: false,
            detail: if installed {
                "虚拟声卡已就绪"
            } else if disabled {
                "虚拟声卡设备已禁用，请启用设备；不会重复安装驱动"
            } else if partial {
                "虚拟声卡端点不完整或不可用，需要检查驱动状态；不会重复安装"
            } else {
                "未检测到可用的 VB-CABLE 输入和输出端点"
            }
            .into(),
            render_endpoint_id: render.map(|d| d.id.clone()),
            capture_endpoint_id: capture.map(|d| d.id.clone()),
            diagnostic: None,
        }
    }
}

fn is_cable_input(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable input")
        || lower.contains("cable in ")
        || (name.contains("扬声器") && name.contains("VB-Audio"))
}

fn is_cable_output(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable output") || (name.contains("麦克风") && name.contains("VB-Audio"))
}

pub fn enumerate_audio_devices(bridge: &Path) -> Result<Vec<LocalAudioDevice>, &'static str> {
    let bytes = run_bounded(
        Command::new(bridge).arg("--list-audio-devices"),
        Duration::from_secs(5),
        64 * 1024,
    )?;
    serde_json::from_slice(&bytes).map_err(|_| "AUDIO_ENUMERATION_FAILED")
}

pub fn run_script_bounded(
    script: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<serde_json::Value, &'static str> {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .args(arguments);
    let bytes = run_bounded(&mut command, timeout, 64 * 1024)?;
    let output = String::from_utf8_lossy(&bytes);
    let result: serde_json::Value = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line.trim()).ok())
        .ok_or("PREREQUISITE_RESULT_INVALID")?;
    if result["installed"] == false || result["success"] == false {
        return Err(
            known_script_error(result["errorCode"].as_str().unwrap_or(""))
                .unwrap_or("PREREQUISITE_INSTALL_FAILED"),
        );
    }
    Ok(result)
}

pub fn run_script_success_bounded(
    script: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<(), &'static str> {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .args(arguments);
    run_bounded(&mut command, timeout, 64 * 1024).map(|_| ())
}

fn run_bounded(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> Result<Vec<u8>, &'static str> {
    run_bounded_observed(command, timeout, max_output, &mut |_| {})
}

pub fn run_preparation_script(
    script: &Path,
    arguments: &[&str],
    timeout: Duration,
    on_phase: &mut dyn FnMut(PreparationEvent),
) -> Result<Option<serde_json::Value>, &'static str> {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .args(arguments);
    let bytes = run_bounded_observed(&mut command, timeout, 64 * 1024, on_phase)?;
    let value = String::from_utf8_lossy(&bytes)
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value.get("installed").is_some() || value.get("success").is_some());
    if let Some(value) = &value
        && (value["installed"] == false || value["success"] == false)
    {
        return Err(
            known_script_error(value["errorCode"].as_str().unwrap_or(""))
                .unwrap_or("PREREQUISITE_INSTALL_FAILED"),
        );
    }
    Ok(value)
}

fn run_bounded_observed(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
    on_phase: &mut dyn FnMut(PreparationEvent),
) -> Result<Vec<u8>, &'static str> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|_| "PREREQUISITE_PROCESS_FAILED")?;
    let stdout = child.stdout.take().ok_or("PREREQUISITE_PROCESS_FAILED")?;
    let stderr = child.stderr.take().ok_or("PREREQUISITE_PROCESS_FAILED")?;
    let (phase_send, phases) = std::sync::mpsc::sync_channel(16);
    let reader = read_pipe_bounded(stdout, max_output, Some(phase_send));
    let error_reader = read_pipe_bounded(stderr, max_output, None);
    let started = Instant::now();
    let mut timed_out = false;
    let mut exit_code = None;
    let success = loop {
        for phase in phases.try_iter() {
            on_phase(PreparationEvent::Phase(phase));
        }
        if started.elapsed() > timeout {
            timed_out = true;
            let _ = child.kill();
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break status.success();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break false,
        }
    };
    let _ = child.wait();
    // An elevated descendant may outlive its wrapper. Never join a reader
    // indefinitely; the installer worker mutex prevents a second installation.
    let bytes = reader
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let errors = error_reader
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    for phase in phases.try_iter() {
        on_phase(PreparationEvent::Phase(phase));
    }
    on_phase(PreparationEvent::Exit(exit_code));
    if timed_out {
        return Err("PREREQUISITE_TIMEOUT");
    }
    if bytes.len() > max_output || errors.len() > max_output {
        return Err("PREREQUISITE_OUTPUT_TOO_LARGE");
    }
    if !success {
        return Err(known_script_error(&String::from_utf8_lossy(&bytes))
            .or_else(|| known_script_error(&String::from_utf8_lossy(&errors)))
            .unwrap_or("PREREQUISITE_PROCESS_FAILED"));
    }
    Ok(bytes)
}

pub(crate) fn read_pipe_bounded(
    pipe: impl std::io::Read + Send + 'static,
    limit: usize,
    phases: Option<std::sync::mpsc::SyncSender<String>>,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut line = Vec::new();
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let keep = count.min((limit + 1).saturating_sub(bytes.len()));
            bytes.extend_from_slice(&buffer[..keep]);
            if let Some(phases) = &phases {
                for byte in &buffer[..count] {
                    if *byte == b'\n' {
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line)
                            && let Some(phase) = value["phase"].as_str()
                            && matches!(
                                phase,
                                "downloading" | "verifying" | "authorizing" | "installing"
                            )
                        {
                            let _ = phases.try_send(phase.into());
                        }
                        line.clear();
                    } else if line.len() < 1024 {
                        line.push(*byte);
                    }
                }
            }
        }
        let _ = send.send(bytes);
    });
    receive
}

// Only fixed, reviewed codes leave the process boundary. Never surface arbitrary
// stderr (which may contain local paths, environment values or upstream data).
fn known_script_error(text: &str) -> Option<&'static str> {
    [
        "PREREQUISITE_UAC_CANCELLED",
        "PREREQUISITE_HASH_MISMATCH",
        "PREREQUISITE_SIGNATURE_REJECTED",
        "PREREQUISITE_DOWNLOAD_FAILED",
        "PREREQUISITE_MODULE_LOAD_FAILED",
        "PREREQUISITE_RESOURCE_MISSING",
        "PREREQUISITE_INSTALL_BUSY",
        "PREREQUISITE_TIMEOUT",
        "PREREQUISITE_DEVICE_DISABLED",
        "PREREQUISITE_INSTALL_FAILED",
    ]
    .into_iter()
    .find(|code| {
        text.split(|c: char| !(c.is_ascii_uppercase() || c == '_'))
            .any(|token| token == *code)
    })
}

pub fn preparation_error_message(code: &str) -> &'static str {
    match code {
        "PREREQUISITE_UAC_CANCELLED" => "已取消管理员授权，尚未完成安装。可以重新点击安装。",
        "PREREQUISITE_HASH_MISMATCH" => "安装包校验失败，未运行安装程序。请重新下载。",
        "PREREQUISITE_SIGNATURE_REJECTED" => {
            "安装程序签名验证失败，已阻止安装。请检查系统时间和证书信任。"
        }
        "PREREQUISITE_DOWNLOAD_FAILED" => "安装包下载失败，请检查网络后重试。",
        "PREREQUISITE_MODULE_LOAD_FAILED" => "Windows PowerShell 安全模块无法加载，尚未启动安装。",
        "PREREQUISITE_RESOURCE_MISSING" => "缺少安装脚本或安装文件，请修复开发环境中的音频组件。",
        "PREREQUISITE_INSTALL_BUSY" => {
            "安装任务仍在运行，请等待管理员授权或安装窗口完成，不要重复安装。"
        }
        "PREREQUISITE_TIMEOUT" => {
            "安装等待超时，提权任务可能仍在运行。请先检查状态，不要重复安装。"
        }
        "PREREQUISITE_DEVICE_DISABLED" => "虚拟声卡设备已禁用，不会重复安装驱动。请先启用设备。",
        "PREREQUISITE_RESULT_INVALID" => "安装组件返回了无法识别的结果，未确认安装成功。",
        "PREREQUISITE_OUTPUT_TOO_LARGE" => "安装组件输出超出上限，未确认安装成功。",
        "PREREQUISITE_PROCESS_FAILED" => "安装进程异常退出，未确认安装成功。",
        _ => "音频准备失败，未确认安装成功。请查看当前失败阶段后重试。",
    }
}

pub fn preparation_phase_label(phase: &str) -> &'static str {
    match phase {
        "checking" => "检测安装环境",
        "downloading" => "下载安装包",
        "verifying" => "校验安装包和签名",
        "authorizing" => "等待 Windows 管理员授权",
        "installing" => "安装驱动",
        "rechecking" => "重新检测音频端点",
        _ => "音频准备",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryView {
    Bits32,
    Bits64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryQueryResult {
    pub status: i32,
    pub stdout: String,
}

pub trait CameraRegistryProbe {
    fn query(&self, view: RegistryView) -> &RegistryQueryResult;
}

pub trait AudioDeviceProbe {
    fn enum_output(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedCameraProbe {
    pub bits32: RegistryQueryResult,
    pub bits64: RegistryQueryResult,
}

impl CameraRegistryProbe for InjectedCameraProbe {
    fn query(&self, view: RegistryView) -> &RegistryQueryResult {
        match view {
            RegistryView::Bits32 => &self.bits32,
            RegistryView::Bits64 => &self.bits64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedAudioProbe {
    pub output: String,
}

impl AudioDeviceProbe for InjectedAudioProbe {
    fn enum_output(&self) -> &str {
        &self.output
    }
}

pub fn obs64_executable(root: &Path) -> PathBuf {
    root.join("bin").join("64bit").join("obs64.exe")
}

pub fn owned_obs_root(paths: &ResolvedPaths) -> &Path {
    if obs64_executable(&paths.obs_runtime).is_file() {
        &paths.obs_runtime
    } else {
        &paths.obs_template
    }
}

pub fn is_obs_bundled(paths: &ResolvedPaths) -> bool {
    obs64_executable(&paths.obs_template).is_file()
        || obs64_executable(&paths.obs_runtime).is_file()
}

pub fn registry_value_references_module(output: &str, module_path: &Path) -> bool {
    let expected = normalize_windows_path(&module_path.to_string_lossy());
    output.lines().any(|line| {
        let Some(offset) = line.to_ascii_uppercase().find("REG_SZ") else {
            return false;
        };
        normalize_windows_path(&line[offset + "REG_SZ".len()..]) == expected
    })
}

pub fn is_obs_virtual_camera_registered(obs_root: &Path, probe: &impl CameraRegistryProbe) -> bool {
    [RegistryView::Bits64, RegistryView::Bits32]
        .into_iter()
        .all(|view| {
            let result = probe.query(view);
            result.status == 0
                && registry_value_references_module(
                    &result.stdout,
                    &virtual_camera_module(obs_root, view),
                )
        })
}

pub fn is_vb_cable_pair_present(output: &str) -> bool {
    let recording =
        contains_ascii_word(output, "cable output") || has_vb_audio_label(output, "麦克风");
    let playback = contains_ascii_word(output, "cable input")
        || contains_ascii_word(output, "cable in")
        || has_vb_audio_label(output, "扬声器");
    recording && playback
}

pub fn report_prerequisite_status(
    paths: &ResolvedPaths,
    camera: &impl CameraRegistryProbe,
    audio: &impl AudioDeviceProbe,
) -> PrerequisiteStatus {
    PrerequisiteStatus {
        obs_bundled: is_obs_bundled(paths),
        virtual_camera_registered: is_obs_virtual_camera_registered(owned_obs_root(paths), camera),
        virtual_audio_installed: is_vb_cable_pair_present(audio.enum_output()),
    }
}

fn virtual_camera_module(obs_root: &Path, view: RegistryView) -> PathBuf {
    obs_root
        .join("data")
        .join("obs-plugins")
        .join("win-dshow")
        .join(view.module_file())
}

fn normalize_windows_path(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.replace('/', "\\").to_ascii_lowercase()
}

fn contains_ascii_word(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(offset) = haystack[start..].find(&needle) {
        let index = start + offset;
        let before_ok = index == 0 || !is_ascii_word_byte(bytes[index - 1]);
        let after = index + needle.len();
        let after_ok = after == bytes.len() || !is_ascii_word_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = index + 1;
    }
    false
}

fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn has_vb_audio_label(output: &str, label: &str) -> bool {
    let mut search = output;
    while let Some(index) = search.find(label) {
        let after = search[index + label.len()..].trim_start();
        if let Some(inner) = after.strip_prefix('(')
            && let Some(end) = inner.find(')')
            && inner[..end].contains("VB-Audio")
        {
            return true;
        }
        search = &search[index + label.len()..];
    }
    false
}

impl RegistryView {
    pub const fn module_file(self) -> &'static str {
        match self {
            Self::Bits32 => "obs-virtualcam-module32.dll",
            Self::Bits64 => "obs-virtualcam-module64.dll",
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(windows)]
    fn preparation_reports_only_allowed_phases_and_times_out() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("phases.ps1");
        std::fs::write(&script, "Write-Output '{\"phase\":\"verifying\"}'; Write-Output '{\"phase\":\"private-data\"}'; Write-Output '{\"phase\":\"installing\"}'; Start-Sleep -Seconds 120").unwrap();
        let mut phases = Vec::new();
        // 30s: powershell.exe cold start under full-suite parallel load has been
        // observed to exceed 5s on CI runners (Defender scan + .NET JIT); the
        // script keeps sleeping for 120s so the timeout still fires after every
        // phase has been emitted.
        let result = super::run_preparation_script(
            &script,
            &[],
            std::time::Duration::from_secs(30),
            &mut |event| {
                if let super::PreparationEvent::Phase(phase) = event {
                    phases.push(phase)
                }
            },
        );
        assert_eq!(result.unwrap_err(), "PREREQUISITE_TIMEOUT");
        assert_eq!(phases, ["verifying", "installing"]);
    }

    #[test]
    #[cfg(windows)]
    fn oversized_installer_output_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("huge.ps1");
        std::fs::write(
            &script,
            "$payload = 'A' * 70000; Write-Output $payload; exit 0",
        )
        .unwrap();
        assert_eq!(
            super::run_script_success_bounded(&script, &[], std::time::Duration::from_secs(30))
                .unwrap_err(),
            "PREREQUISITE_OUTPUT_TOO_LARGE"
        );
    }

    #[test]
    #[cfg(windows)]
    fn timeout_then_live_worker_lock_blocks_reentry() {
        use std::io::Read;
        let directory = tempfile::tempdir().unwrap();
        let hang = directory.path().join("hang.ps1");
        std::fs::write(&hang, "Start-Sleep -Seconds 20").unwrap();
        let holder_code = String::from(
            "$m=[Threading.Mutex]::new($false,'Local\\AI.VirtualAssistant.VirtualAudio.Worker'); $acquired=$m.WaitOne(); [Console]::Out.WriteLine('locked'); [Console]::Out.Flush(); [Console]::In.ReadLine() | Out-Null; if($acquired){ $m.ReleaseMutex() }; $m.Dispose()",
        );
        let mut holder = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &holder_code,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = holder.stdout.take().unwrap();
        let mut buf = [0_u8; 16];
        stdout.read_exact(&mut buf[..6]).unwrap();
        let timed_out = super::run_preparation_script(
            &hang,
            &[],
            std::time::Duration::from_secs(1),
            &mut |_| {},
        );
        assert_eq!(timed_out.unwrap_err(), "PREREQUISITE_TIMEOUT");
        let install = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/install-prerequisite.ps1");
        let probe = super::run_script_bounded(
            &install,
            &[
                "-Component",
                "virtual-audio",
                "-ResourcesDirectory",
                &directory.path().to_string_lossy(),
                "-ProbeOnly",
            ],
            // 30s：同上，CI 负载下 powershell.exe 冷启动可能超过 15s，
            // 超时会误报 PREREQUISITE_TIMEOUT 而非预期的 INSTALL_BUSY。
            std::time::Duration::from_secs(30),
        );
        assert_eq!(probe.unwrap_err(), "PREREQUISITE_INSTALL_BUSY");
        holder.stdin.take();
        let _ = holder.wait();
    }

    #[test]
    fn audio_routing_record_round_trips_device_ids_without_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let change = super::AudioRoutingChange {
            bridge: directory.path().join("AudioBridge.exe"),
            previous_id: "{0.0.1.00000000}.{abcd}".into(),
            cable_id: "{0.0.1.00000000}.{cable}".into(),
            changed: true,
            previous_console_id: Some("{0.0.1.00000000}.{ef01}".into()),
            previous_multimedia_id: Some("{0.0.1.00000000}.{ef02}".into()),
        };
        super::persist_audio_routing(directory.path(), &change).unwrap();
        let loaded = super::load_persisted_audio_routing(directory.path()).unwrap();
        assert_eq!(loaded.previous_id, change.previous_id);
        assert_eq!(loaded.cable_id, change.cable_id);
        assert_eq!(loaded.previous_console_id, change.previous_console_id);
        assert_eq!(loaded.previous_multimedia_id, change.previous_multimedia_id);
        let encoded = serde_json::to_string(&loaded).unwrap();
        assert!(!encoded.contains("sk-"));
        super::clear_persisted_audio_routing(directory.path());
        assert!(super::load_persisted_audio_routing(directory.path()).is_none());
    }

    #[test]
    fn legacy_routing_record_without_role_fields_still_loads() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("prerequisites")).unwrap();
        std::fs::write(
            directory.path().join("prerequisites/audio-routing.json"),
            r#"{"bridge":"AudioBridge.exe","previous_id":"old-mic","cable_id":"cable","changed":true}"#,
        )
        .unwrap();
        let loaded = super::load_persisted_audio_routing(directory.path()).unwrap();
        assert_eq!(loaded.previous_id, "old-mic");
        assert_eq!(loaded.previous_console_id, None);
        assert_eq!(loaded.previous_multimedia_id, None);
    }

    #[test]
    fn capture_mic_change_json_parses_all_three_roles() {
        let bytes = r#"{"changed":true,"cableId":"{cable}","cableLabel":"麦克风 (VB-Audio Virtual Cable)","roles":[
            {"role":"Console","changed":true,"previousId":"{console}"},
            {"role":"Multimedia","changed":true,"previousId":"{multimedia}"},
            {"role":"Communications","changed":true,"previousId":"{communications}"}]}"#;
        let change = super::parse_capture_mic_change(
            std::path::Path::new("AudioBridge.exe"),
            bytes.as_bytes(),
        )
        .expect("parse");
        assert!(change.changed);
        assert_eq!(change.cable_id, "{cable}");
        assert_eq!(change.previous_id, "{communications}");
        assert_eq!(change.previous_console_id.as_deref(), Some("{console}"));
        assert_eq!(
            change.previous_multimedia_id.as_deref(),
            Some("{multimedia}")
        );
    }

    #[test]
    fn capture_mic_change_json_rejects_missing_role_entries() {
        let bytes = br#"{"changed":false,"cableId":"{cable}","cableLabel":"x","roles":[
            {"role":"Console","changed":false,"previousId":"{cable}"}]}"#;
        assert_eq!(
            super::parse_capture_mic_change(std::path::Path::new("AudioBridge.exe"), bytes),
            Err("AUDIO_ROUTING_RESULT_INVALID")
        );
    }

    #[test]
    fn disabled_cable_is_not_reported_missing_or_ready() {
        let devices: Vec<super::LocalAudioDevice> = serde_json::from_str(r#"[{"id":"r","name":"CABLE Input","flow":"render","state":"Disabled"},{"id":"c","name":"CABLE Output","flow":"capture","state":"Active"}]"#).unwrap();
        let result = super::VirtualAudioPreparation::from_devices(&devices);
        assert_eq!(result.state, "disabled");
        assert!(!result.installed);
    }
    #[test]
    #[cfg(windows)]
    fn installer_preserves_nonzero_structured_error() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("安装 diagnostic.ps1");
        std::fs::write(&script, "Write-Output '{\"installed\":false,\"errorCode\":\"PREREQUISITE_UAC_CANCELLED\"}'; exit 1").unwrap();
        assert_eq!(
            super::run_script_bounded(&script, &[], std::time::Duration::from_secs(30))
                .unwrap_err(),
            "PREREQUISITE_UAC_CANCELLED"
        );
    }

    #[test]
    #[cfg(windows)]
    fn installer_preserves_download_error_from_stderr() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fetch.ps1");
        std::fs::write(&script, "[Console]::Error.WriteLine('PREREQUISITE_HASH_MISMATCH: private path must not escape'); exit 1").unwrap();
        assert_eq!(
            super::run_script_success_bounded(&script, &[], std::time::Duration::from_secs(30))
                .unwrap_err(),
            "PREREQUISITE_HASH_MISMATCH"
        );
    }
    use super::*;
    use crate::obs::{PathRoots, resolve_owned_paths};
    use std::fs;

    fn roots(directory: &tempfile::TempDir) -> PathRoots {
        let repository = directory.path().to_path_buf();
        PathRoots {
            resource_root: repository.join("resources"),
            data_directory: repository.join("data"),
            repository,
            development: true,
        }
    }

    fn resolved_paths(directory: &tempfile::TempDir) -> ResolvedPaths {
        let roots = roots(directory);
        fs::create_dir_all(
            roots
                .resource_root
                .join("prerequisites")
                .join("obs-portable"),
        )
        .unwrap();
        fs::create_dir_all(&roots.data_directory).unwrap();
        resolve_owned_paths(&roots).expect("template directory present")
    }

    fn write_obs64(root: &Path) {
        let executable = obs64_executable(root);
        fs::create_dir_all(executable.parent().expect("obs64 parent")).unwrap();
        fs::write(&executable, []).unwrap();
    }

    fn registry_line(module_path: &Path) -> String {
        format!("    (Default)    REG_SZ    {}\n", module_path.display())
    }

    fn matching_camera_probe(obs_root: &Path) -> InjectedCameraProbe {
        let module_directory = obs_root.join("data").join("obs-plugins").join("win-dshow");
        InjectedCameraProbe {
            bits64: RegistryQueryResult {
                status: 0,
                stdout: registry_line(&module_directory.join(RegistryView::Bits64.module_file())),
            },
            bits32: RegistryQueryResult {
                status: 0,
                stdout: registry_line(&module_directory.join(RegistryView::Bits32.module_file())),
            },
        }
    }

    fn silent_camera() -> InjectedCameraProbe {
        InjectedCameraProbe {
            bits32: RegistryQueryResult {
                status: 1,
                stdout: String::new(),
            },
            bits64: RegistryQueryResult {
                status: 1,
                stdout: String::new(),
            },
        }
    }

    fn silent_audio() -> InjectedAudioProbe {
        InjectedAudioProbe {
            output: String::new(),
        }
    }

    #[test]
    fn obs_bundled_is_false_when_owned_roots_lack_obs64() {
        let directory = tempfile::tempdir().unwrap();
        let paths = resolved_paths(&directory);
        let status = report_prerequisite_status(&paths, &silent_camera(), &silent_audio());
        assert!(!is_obs_bundled(&paths));
        assert!(!status.obs_bundled);
        assert!(!status.virtual_camera_registered);
        assert!(!status.virtual_audio_installed);
    }

    #[test]
    fn obs_bundled_is_true_when_template_has_obs64() {
        let directory = tempfile::tempdir().unwrap();
        let paths = resolved_paths(&directory);
        write_obs64(&paths.obs_template);
        let status = report_prerequisite_status(&paths, &silent_camera(), &silent_audio());
        assert!(status.obs_bundled);
        assert_eq!(owned_obs_root(&paths), paths.obs_template.as_path());
    }

    #[test]
    fn obs_bundled_is_true_when_runtime_has_obs64() {
        let directory = tempfile::tempdir().unwrap();
        let paths = resolved_paths(&directory);
        write_obs64(&paths.obs_runtime);
        let status = report_prerequisite_status(&paths, &silent_camera(), &silent_audio());
        assert!(status.obs_bundled);
        assert_eq!(owned_obs_root(&paths), paths.obs_runtime.as_path());
    }

    #[test]
    fn virtual_camera_registered_follows_injected_registry_probe() {
        let directory = tempfile::tempdir().unwrap();
        let paths = resolved_paths(&directory);
        write_obs64(&paths.obs_template);
        let camera = matching_camera_probe(&paths.obs_template);
        let status = report_prerequisite_status(&paths, &camera, &silent_audio());
        assert!(status.virtual_camera_registered);

        let mismatched = InjectedCameraProbe {
            bits64: camera.bits64.clone(),
            bits32: RegistryQueryResult {
                status: 0,
                stdout: registry_line(Path::new(r"C:\old-preview\obs-virtualcam-module32.dll")),
            },
        };
        let status = report_prerequisite_status(&paths, &mismatched, &silent_audio());
        assert!(!status.virtual_camera_registered);

        let missing_32 = InjectedCameraProbe {
            bits64: camera.bits64,
            bits32: RegistryQueryResult {
                status: 1,
                stdout: String::new(),
            },
        };
        let status = report_prerequisite_status(&paths, &missing_32, &silent_audio());
        assert!(!status.virtual_camera_registered);
    }

    #[test]
    fn registry_parser_is_case_insensitive_and_rejects_suffix_or_injection() {
        let module_path = Path::new(r"C:\OBS Path\obs-virtualcam-module64.dll");
        assert!(registry_value_references_module(
            r#"(Default) REG_SZ "c:\obs path\OBS-VIRTUALCAM-MODULE64.DLL""#,
            module_path,
        ));
        assert!(!registry_value_references_module(
            r"(Default) REG_SZ C:\OBS Path\obs-virtualcam-module64.dll.old",
            module_path,
        ));
        assert!(!registry_value_references_module(
            r"(Default) REG_SZ C:\OBS Path\obs-virtualcam-module64.dll & calc.exe",
            module_path,
        ));
    }

    #[test]
    fn virtual_audio_installed_detects_vb_cable_pair_from_injected_output() {
        let directory = tempfile::tempdir().unwrap();
        let paths = resolved_paths(&directory);
        let cable_pair = "Device Description: CABLE Input (VB-Audio Virtual Cable)\r\nDevice Description: CABLE Output (VB-Audio Virtual Cable)\r\n";
        let cable_pair_16ch = "Device Description: CABLE In 16 Ch (VB-Audio Virtual Cable)\r\nDevice Description: CABLE Output (VB-Audio Virtual Cable)\r\n";
        let cable_pair_chinese = "Device Description: 扬声器 (VB-Audio Virtual Cable)\r\nDevice Description: 麦克风 (VB-Audio Virtual Cable)\r\n";

        assert!(is_vb_cable_pair_present(cable_pair));
        assert!(is_vb_cable_pair_present(cable_pair_16ch));
        assert!(is_vb_cable_pair_present(cable_pair_chinese));
        assert!(!is_vb_cable_pair_present(
            "Device Description: CABLE Output (VB-Audio Virtual Cable)\r\n"
        ));
        assert!(!is_vb_cable_pair_present(
            "Device Description: CABLE In 16 Ch (VB-Audio Virtual Cable)\r\n"
        ));

        let status = report_prerequisite_status(
            &paths,
            &silent_camera(),
            &InjectedAudioProbe {
                output: cable_pair.to_owned(),
            },
        );
        assert!(status.virtual_audio_installed);
        assert_eq!(
            OBS_VIRTUAL_CAMERA_CLSID,
            "{A3FCE0F5-3493-419F-958A-ABA1250EC20B}"
        );
    }

    #[test]
    fn virtual_audio_requires_a_render_and_capture_pair() {
        let render = LocalAudioDevice {
            id: "r".into(),
            name: "CABLE Input (VB-Audio Virtual Cable)".into(),
            flow: "render".into(),
            state: "Active".into(),
        };
        let capture = LocalAudioDevice {
            id: "c".into(),
            name: "CABLE Output (VB-Audio Virtual Cable)".into(),
            flow: "capture".into(),
            state: "Active".into(),
        };
        assert!(!VirtualAudioPreparation::from_devices(std::slice::from_ref(&render)).installed);
        let ready = VirtualAudioPreparation::from_devices(&[render, capture]);
        assert!(ready.installed);
        assert_eq!(ready.render_endpoint_id.as_deref(), Some("r"));
        assert_eq!(ready.capture_endpoint_id.as_deref(), Some("c"));
    }
}
