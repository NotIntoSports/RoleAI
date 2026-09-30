//! Meeting-process allowlist and capture gate. Tests inject the enumerator.

use std::sync::{
    Mutex,
    atomic::{AtomicU32, Ordering},
};
#[cfg(windows)]
use std::{
    io::Read,
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const PROCESS_SNAPSHOT_MAX_BYTES: usize = 1024 * 1024;

pub const MEETING_EXECUTABLE_NAMES: &[&str] = &[
    "teams.exe",
    "ms-teams.exe",
    "wemeetapp.exe",
    "feishu.exe",
    "lark.exe",
    "dingtalk.exe",
    "zoom.exe",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct MeetingProcess {
    pub pid: u32,
    pub name: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    EnumerationFailed,
    SnapshotInvalid,
    /// 当前平台不支持会议进程采集（仅非 Windows 的生产枚举器返回）。
    PlatformUnsupported,
}

impl ProcessError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::EnumerationFailed => "MEETING_PROCESS_ENUM_FAILED",
            Self::SnapshotInvalid => "MEETING_PROCESS_SNAPSHOT_INVALID",
            Self::PlatformUnsupported => "PLATFORM_UNSUPPORTED",
        }
    }
}

pub trait ProcessEnumerator {
    fn list(&self) -> Result<Vec<MeetingProcess>, ProcessError>;
}

#[derive(Debug, Default)]
pub struct InjectedProcessEnumerator {
    processes: Mutex<Vec<MeetingProcess>>,
    calls: AtomicU32,
}

impl InjectedProcessEnumerator {
    pub fn new(processes: Vec<MeetingProcess>) -> Self {
        Self {
            processes: Mutex::new(processes),
            calls: AtomicU32::new(0),
        }
    }

    pub fn set(&self, processes: Vec<MeetingProcess>) {
        *self.lock() = processes;
    }

    pub fn call_count(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<MeetingProcess>> {
        self.processes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProcessEnumerator for InjectedProcessEnumerator {
    fn list(&self) -> Result<Vec<MeetingProcess>, ProcessError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.lock().clone())
    }
}

#[derive(Debug, Default)]
pub struct FailingProcessEnumerator;

impl ProcessEnumerator for FailingProcessEnumerator {
    fn list(&self) -> Result<Vec<MeetingProcess>, ProcessError> {
        Err(ProcessError::EnumerationFailed)
    }
}

pub fn is_allowlisted_executable(name: &str) -> bool {
    MEETING_EXECUTABLE_NAMES
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

pub fn filter_meeting_processes(processes: Vec<MeetingProcess>) -> Vec<MeetingProcess> {
    let mut listed: Vec<MeetingProcess> = processes
        .into_iter()
        .filter(|process| {
            process.pid > 0
                && !process.title.trim().is_empty()
                && is_allowlisted_executable(&process.name)
        })
        .collect();
    listed.sort_by(|left, right| left.name.cmp(&right.name).then(left.pid.cmp(&right.pid)));
    listed
}

pub fn list_meeting_processes(
    enumerator: &(impl ProcessEnumerator + ?Sized),
) -> Result<Vec<MeetingProcess>, ProcessError> {
    Ok(filter_meeting_processes(enumerator.list()?))
}

pub fn parse_process_snapshot(raw: &str) -> Result<Vec<MeetingProcess>, ProcessError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| ProcessError::SnapshotInvalid)?;
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(_) => vec![value],
        _ => return Err(ProcessError::SnapshotInvalid),
    };
    Ok(rows.iter().filter_map(process_from_json).collect())
}

pub fn set_default_communications_mic_args(capture_id: &str) -> Vec<String> {
    vec!["--set-default-communications-mic".into(), capture_id.into()]
}

pub fn restore_default_communications_mic_args(
    endpoint_id: &str,
    expected_id: &str,
) -> Vec<String> {
    vec![
        "--restore-default-communications-mic".into(),
        endpoint_id.to_string(),
        expected_id.to_string(),
    ]
}

pub fn meeting_process_powershell_script() -> &'static str {
    "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); Get-Process | Where-Object { $_.MainWindowTitle } | Select-Object @{n='pid';e={$_.Id}},@{n='name';e={$_.Path | Split-Path -Leaf}},@{n='title';e={$_.MainWindowTitle}} | ConvertTo-Json -Compress"
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PowerShellProcessEnumerator;

impl ProcessEnumerator for PowerShellProcessEnumerator {
    fn list(&self) -> Result<Vec<MeetingProcess>, ProcessError> {
        #[cfg(windows)]
        {
            enumerate_via_powershell()
        }
        #[cfg(not(windows))]
        {
            // 非 Windows 没有白名单内的会议进程采集面：返回稳定错误码，
            // 前端据此隐藏会议采集入口（而非展示空列表）。
            Err(ProcessError::PlatformUnsupported)
        }
    }
}

fn process_from_json(value: &serde_json::Value) -> Option<MeetingProcess> {
    let object = value.as_object()?;
    let pid = object.get("pid")?.as_u64()?;
    let pid = u32::try_from(pid).ok()?;
    let name = object.get("name")?.as_str()?.to_string();
    let title = object.get("title")?.as_str()?.to_string();
    Some(MeetingProcess { pid, name, title })
}

#[cfg(windows)]
fn hide_windows_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(windows)]
fn enumerate_via_powershell() -> Result<Vec<MeetingProcess>, ProcessError> {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            meeting_process_powershell_script(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_windows_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| ProcessError::EnumerationFailed)?;
    let mut stdout = child.stdout.take().ok_or(ProcessError::EnumerationFailed)?;
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if raw.len().saturating_add(n) > PROCESS_SNAPSHOT_MAX_BYTES {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProcessError::SnapshotInvalid);
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProcessError::EnumerationFailed);
            }
        }
    }
    let status = child.wait().map_err(|_| ProcessError::EnumerationFailed)?;
    if !status.success() {
        return Err(ProcessError::EnumerationFailed);
    }
    let text = std::str::from_utf8(&raw).map_err(|_| ProcessError::SnapshotInvalid)?;
    parse_process_snapshot(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, title: &str) -> MeetingProcess {
        MeetingProcess {
            pid,
            name: name.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn lists_visible_allowlisted_processes_and_keeps_duplicates() {
        let enumerator = InjectedProcessEnumerator::new(vec![
            process(10, "Teams.exe", "Interview"),
            process(11, "Teams.exe", "Second call"),
            process(12, "notepad.exe", "Notes"),
            process(13, "WeMeetApp.exe", ""),
            process(14, "zoom.exe", "Standup"),
            process(0, "zoom.exe", "Ghost"),
        ]);

        let listed = list_meeting_processes(&enumerator).expect("injected list");
        assert_eq!(
            listed,
            vec![
                process(10, "Teams.exe", "Interview"),
                process(11, "Teams.exe", "Second call"),
                process(14, "zoom.exe", "Standup"),
            ]
        );
        assert_eq!(enumerator.call_count(), 1);
    }

    #[test]
    fn allowlist_is_exact_exe_names_case_insensitive() {
        assert!(is_allowlisted_executable("teams.exe"));
        assert!(is_allowlisted_executable("MS-Teams.EXE"));
        assert!(is_allowlisted_executable("wemeetapp.exe"));
        assert!(is_allowlisted_executable("feishu.exe"));
        assert!(is_allowlisted_executable("lark.exe"));
        assert!(is_allowlisted_executable("dingtalk.exe"));
        assert!(is_allowlisted_executable("zoom.exe"));
        assert!(!is_allowlisted_executable("ms-teams-helper.exe"));
        assert!(!is_allowlisted_executable("zoom"));
        assert!(!is_allowlisted_executable("notepad.exe"));
        assert_eq!(MEETING_EXECUTABLE_NAMES.len(), 7);
    }

    #[test]
    fn sorts_allowlisted_processes_by_name_then_pid() {
        let enumerator = InjectedProcessEnumerator::new(vec![
            process(20, "zoom.exe", "B"),
            process(8, "Teams.exe", "A"),
            process(9, "teams.exe", "C"),
        ]);
        let listed = list_meeting_processes(&enumerator).expect("injected list");
        assert_eq!(
            listed.iter().map(|item| item.pid).collect::<Vec<_>>(),
            vec![8, 9, 20]
        );
    }

    #[test]
    fn parse_process_snapshot_accepts_array_object_and_empty() {
        assert_eq!(parse_process_snapshot("").expect("empty"), Vec::new());
        assert_eq!(parse_process_snapshot("   ").expect("blank"), Vec::new());
        let single = parse_process_snapshot(r#"{"pid":42,"name":"zoom.exe","title":"Weekly"}"#)
            .expect("object");
        assert_eq!(single, vec![process(42, "zoom.exe", "Weekly")]);
        let many = parse_process_snapshot(
            r#"[{"pid":1,"name":"teams.exe","title":"A"},{"pid":2,"name":"zoom.exe","title":"B"}]"#,
        )
        .expect("array");
        assert_eq!(
            many,
            vec![process(1, "teams.exe", "A"), process(2, "zoom.exe", "B"),]
        );
    }

    #[test]
    fn parse_process_snapshot_rejects_invalid_json() {
        let error = parse_process_snapshot("{").expect_err("invalid");
        assert_eq!(error, ProcessError::SnapshotInvalid);
        assert_eq!(error.code(), "MEETING_PROCESS_SNAPSHOT_INVALID");
    }

    #[test]
    fn communications_mic_args_match_csharp_cli() {
        assert_eq!(
            set_default_communications_mic_args("cable"),
            ["--set-default-communications-mic", "cable"]
        );
        assert_eq!(
            restore_default_communications_mic_args("endpoint-1", "cable"),
            [
                "--restore-default-communications-mic",
                "endpoint-1",
                "cable"
            ]
        );
    }

    #[test]
    fn powershell_script_is_windowed_json_snapshot() {
        let script = meeting_process_powershell_script();
        assert!(script.contains("Get-Process"));
        assert!(script.contains("MainWindowTitle"));
        assert!(script.contains("ConvertTo-Json"));
        assert!(
            script.contains("[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)")
        );
        let _ = PowerShellProcessEnumerator;
    }

    #[test]
    fn chinese_meeting_title_round_trips_without_replacement_characters() {
        let rows = parse_process_snapshot(
            r#"{"pid":20220,"name":"Feishu.exe","title":"飞书 · 产品评审会议"}"#,
        )
        .unwrap();
        assert_eq!(rows[0].title, "飞书 · 产品评审会议");
    }
}
