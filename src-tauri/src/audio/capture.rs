//! AudioBridge sidecar wrap. Tests inject PCM; live spawn is skipped when the exe is absent.

use std::{
    fmt,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use super::barge_in::BargeInMonitor;
use super::pcm::{PcmRing, downsample_48k_to_16k};
use super::segmenter::{SpeechSegmenter, default_segmenter};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioError {
    ExeMissing,
    InvalidPid,
    ProcessNotAvailable,
    SpawnFailed,
    SidecarFailed,
}

impl AudioError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ExeMissing => "SESSION_SIDECAR_MISSING",
            Self::InvalidPid => "SESSION_SIDECAR_INVALID_PID",
            Self::ProcessNotAvailable => "MEETING_PROCESS_NOT_AVAILABLE",
            Self::SpawnFailed => "SESSION_SIDECAR_SPAWN_FAILED",
            Self::SidecarFailed => "SESSION_SIDECAR_FAILED",
        }
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for AudioError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarPoll {
    Alive,
    Exited,
}

pub trait PlaybackSink {
    fn play_pcm(&mut self, pcm: &[u8], sample_rate: u32);
    fn cancel(&mut self);
}

#[derive(Debug, Default)]
pub struct NoopSink;

impl PlaybackSink for NoopSink {
    fn play_pcm(&mut self, _pcm: &[u8], _sample_rate: u32) {}

    fn cancel(&mut self) {}
}

#[derive(Debug, Default)]
pub struct RecordingSink {
    frames: Vec<u8>,
    sample_rate: Option<u32>,
    cancelled: bool,
}

impl RecordingSink {
    pub fn recorded(&self) -> &[u8] {
        &self.frames
    }

    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled
    }
}

impl PlaybackSink for RecordingSink {
    fn play_pcm(&mut self, pcm: &[u8], sample_rate: u32) {
        self.frames.extend_from_slice(pcm);
        self.sample_rate = Some(sample_rate);
    }

    fn cancel(&mut self) {
        self.cancelled = true;
    }
}

pub fn bridge_command_args(pid: u32) -> Vec<String> {
    vec!["--pid".into(), pid.to_string()]
}

pub fn parse_level_peak(line: &str) -> Option<f64> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("type")?.as_str()? != "level" {
        return None;
    }
    value.get("peak")?.as_f64()
}

/// 实时会话采集侧信号：本地分段/打断判定结果，由会话层转发给实时泵。
/// （DashScope Manual 模式服务端 VAD 已禁用，说完提交与播报打断都靠本地判定。）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeCaptureSignal {
    /// 分段器判定一句话说完：泵应向服务端提交音频缓冲。
    CommitTurn,
    /// 打断监听触发：泵应清空播放并取消在途响应。
    BargeIn,
}

#[derive(Debug)]
struct CaptureState {
    ring: PcmRing,
    segmenter: Box<dyn SpeechSegmenter>,
    last_peak: f64,
    echo_until: Option<Instant>,
    barge_in: Option<BargeInMonitor>,
    barge_flag: Arc<AtomicBool>,
    barge_utterance: Option<Vec<u8>>,
    /// 实时上行 tap：实时会话泵订阅原始 PCM（有界通道，满则丢弃并计数）。
    tap: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    tap_dropped: std::sync::atomic::AtomicU64,
    /// 实时会话信号接收端：分段/打断判定经此转发给泵（None 表示无实时路线）。
    realtime_sink: Option<std::sync::mpsc::Sender<RealtimeCaptureSignal>>,
    /// AGC 当前增益（进分段器/tap 的链路；barge 监听保持原始电平）。
    agc_gain: f32,
}

/// ingest 后取走本地判定信号并发往 sink；须在 capture 锁外调用（send 可能
/// 触发泵侧工作，且重复加锁路径要避免嵌套）。
fn drain_realtime_signals(state: &Mutex<CaptureState>) {
    let sink = state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .realtime_sink
        .clone();
    let Some(sink) = sink else { return };
    let signals = {
        let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
        let barge = state.barge_utterance.take();
        if barge.is_some() {
            // 复位旗标，允许监听器在下一次播报中继续触发。
            state
                .barge_flag
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
        let mut signals = Vec::new();
        if barge.is_some() {
            signals.push(RealtimeCaptureSignal::BargeIn);
        }
        // 段落音频服务端已随流收到，本地只借判定结果，数据直接丢弃。
        while state.segmenter.take().is_some() {
            signals.push(RealtimeCaptureSignal::CommitTurn);
        }
        signals
    };
    for signal in signals {
        let _ = sink.send(signal);
    }
}

/// 无锁热路径句柄：会话期间克隆进 AppState，`session_push_mic_pcm` 直接推送，
/// 不经过 sessions 互斥锁（生成/播报期间麦克风数据不再被丢）。
#[derive(Clone)]
pub struct MicIngestHandle {
    state: Arc<Mutex<CaptureState>>,
}

impl MicIngestHandle {
    pub fn push_pcm(&self, pcm: &[u8]) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        ingest_into_state(&mut state, pcm);
        drop(state);
        drain_realtime_signals(&self.state);
    }
}

/// 回声抑制窗句柄：实时泵播报期间由回调续窗，使 barge 监听只认真人插话
/// （与级联路线 suppress_echo_for 语义一致，供非 'static 的会话服务外借）。
#[derive(Clone)]
pub struct EchoSuppressHandle {
    state: Arc<Mutex<CaptureState>>,
}

impl EchoSuppressHandle {
    pub fn suppress_for(&self, duration: std::time::Duration) {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .echo_until = Some(Instant::now() + duration);
    }
}

#[derive(Debug)]
pub struct AudioCapture {
    state: Arc<Mutex<CaptureState>>,
    child: Mutex<Option<Child>>,
    sidecar_dead: Arc<AtomicBool>,
    sidecar_epoch: Arc<AtomicU64>,
    spawn: Option<(PathBuf, u32)>,
    restarts: u8,
}

impl AudioCapture {
    pub fn from_injected() -> Self {
        Self::empty()
    }

    /// 会议桥接会话由 AudioBridge sidecar 采集；为 false 时音频来自本机麦克风 IPC。
    pub fn is_meeting_bridge(&self) -> bool {
        self.spawn.is_some()
    }

    #[cfg(test)]
    pub fn mark_meeting_bridge_for_tests(&mut self) {
        self.spawn = Some((PathBuf::from("AudioBridge-tests.exe"), 4242));
    }

    pub fn spawn_bridge(
        exe: &Path,
        pid: u32,
        enumerator: &(impl crate::processes::ProcessEnumerator + ?Sized),
    ) -> Result<Self, AudioError> {
        if pid == 0 {
            return Err(AudioError::InvalidPid);
        }
        let allowed = crate::processes::list_meeting_processes(enumerator)
            .map_err(|_| AudioError::SpawnFailed)?;
        if !allowed.iter().any(|process| process.pid == pid) {
            return Err(AudioError::ProcessNotAvailable);
        }
        if !exe.is_file() {
            return Err(AudioError::ExeMissing);
        }
        let mut capture = Self::empty();
        capture.spawn = Some((exe.to_path_buf(), pid));
        capture.start_child(exe, pid)?;
        Ok(capture)
    }

    pub fn push_pcm(&mut self, pcm: &[u8]) {
        {
            let mut state = self.lock();
            ingest_into_state(&mut state, pcm);
        }
        drain_realtime_signals(&self.state);
    }

    /// 无锁热路径句柄：会话开始时由 AppState 持有，IPC 推流不再等 sessions 锁。
    pub fn mic_ingest_handle(&self) -> MicIngestHandle {
        MicIngestHandle {
            state: Arc::clone(&self.state),
        }
    }

    /// 订阅原始 PCM tap（实时会话上行）；返回旧 tap。容量 50 帧 ≈ 5s@100ms。
    pub fn set_pcm_tap(
        &self,
        tap: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    ) -> Option<std::sync::mpsc::SyncSender<Vec<u8>>> {
        let mut state = self.lock();
        std::mem::replace(&mut state.tap, tap)
    }

    /// 注册实时会话信号接收端（分段/打断判定转发给泵）。None 摘除。
    pub fn set_realtime_sink(&self, sink: Option<std::sync::mpsc::Sender<RealtimeCaptureSignal>>) {
        self.lock().realtime_sink = sink;
    }

    /// 外借回声抑制窗句柄（实时泵播报续窗用）。
    pub fn echo_suppress_handle(&self) -> EchoSuppressHandle {
        EchoSuppressHandle {
            state: Arc::clone(&self.state),
        }
    }

    #[cfg(test)]
    pub fn stage_barge_utterance_for_tests(&self, pcm: Vec<u8>) {
        self.lock().barge_utterance = Some(pcm);
    }

    /// 测试专用：注入固定概率的 VAD 替身替换 barge 监听器，钉住
    /// 「AGC 增益后的信号喂监听」这条链路。
    #[cfg(test)]
    pub fn stage_barge_monitor_for_tests(
        &self,
        detector: Box<dyn crate::audio::vad::VoiceActivityDetector>,
    ) {
        self.lock().barge_in = Some(crate::audio::barge_in::BargeInMonitor::with_detector(detector));
    }

    pub fn tap_dropped(&self) -> u64 {
        self.lock()
            .tap_dropped
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn ingest_event_line(&mut self, line: &str) {
        if let Some(peak) = parse_level_peak(line) {
            self.lock().last_peak = peak;
        }
    }

    pub fn last_peak(&self) -> f64 {
        self.lock().last_peak
    }

    pub fn snapshot_48k(&self) -> Vec<u8> {
        self.lock().ring.snapshot()
    }

    pub fn pcm_for_asr(&self) -> Vec<u8> {
        downsample_48k_to_16k(&self.snapshot_48k())
    }

    pub fn utterance_ready(&self) -> bool {
        let state = self.lock();
        state.barge_utterance.is_some() || state.segmenter.ready()
    }

    pub fn take_utterance_for_asr(&self) -> Option<Vec<u8>> {
        let mut state = self.lock();
        if let Some(pcm) = state.barge_utterance.take() {
            return Some(downsample_48k_to_16k(&pcm));
        }
        state
            .segmenter
            .take()
            .map(|pcm| downsample_48k_to_16k(&pcm))
    }

    pub fn set_barge_in_enabled(&self, enabled: bool) {
        let mut state = self.lock();
        state.barge_in = if enabled {
            match crate::audio::vad::SileroVad::new() {
                Ok(vad) => Some(BargeInMonitor::with_detector(Box::new(vad))),
                Err(_) => None,
            }
        } else {
            None
        };
    }

    pub fn barge_in_flag(&self) -> Arc<AtomicBool> {
        self.lock().barge_flag.clone()
    }

    pub fn take_barge_in_utterance(&self) -> Option<Vec<u8>> {
        let mut state = self.lock();
        state.barge_utterance.take()
    }

    pub fn suppress_echo_for(&self, duration: Duration) {
        let mut state = self.lock();
        state.echo_until = Some(Instant::now() + duration);
        state.segmenter.reset();
        state.ring.clear();
    }

    pub fn overrun_count(&self) -> u32 {
        self.lock().ring.overrun_count()
    }

    pub fn poll_sidecar(&self) -> Result<SidecarPoll, AudioError> {
        self.refresh_sidecar_exit();
        if self.sidecar_dead.load(Ordering::SeqCst) {
            Ok(SidecarPoll::Exited)
        } else {
            Ok(SidecarPoll::Alive)
        }
    }

    pub fn restart_once(&mut self) -> Result<(), AudioError> {
        if self.restarts >= 1 {
            return Err(AudioError::SidecarFailed);
        }
        self.restarts += 1;
        if let Some((exe, pid)) = self.spawn.clone() {
            self.stop_child();
            self.start_child(&exe, pid)?;
        }
        self.sidecar_dead.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn empty() -> Self {
        Self {
            state: Arc::new(Mutex::new(CaptureState {
                ring: PcmRing::new(),
                segmenter: default_segmenter(),
                last_peak: 0.0,
                echo_until: None,
                barge_in: None,
                barge_flag: Arc::new(AtomicBool::new(false)),
                barge_utterance: None,
                tap: None,
                tap_dropped: std::sync::atomic::AtomicU64::new(0),
                realtime_sink: None,
                agc_gain: 1.0,
            })),
            child: Mutex::new(None),
            sidecar_dead: Arc::new(AtomicBool::new(false)),
            sidecar_epoch: Arc::new(AtomicU64::new(0)),
            spawn: None,
            restarts: 0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn start_child(&mut self, exe: &Path, pid: u32) -> Result<(), AudioError> {
        let epoch = self.sidecar_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut state = self.lock();
            state.ring.clear();
            state.segmenter.reset();
            state.last_peak = 0.0;
            state.echo_until = None;
        }
        self.sidecar_dead.store(false, Ordering::SeqCst);
        let mut command = Command::new(exe);
        command
            .args(bridge_command_args(pid))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        hide_windows_console(&mut command);
        let mut child = command.spawn().map_err(|_| AudioError::SpawnFailed)?;
        let stdout = child.stdout.take().ok_or(AudioError::SpawnFailed)?;
        let stderr = child.stderr.take().ok_or(AudioError::SpawnFailed)?;
        let pcm_state = Arc::clone(&self.state);
        let pcm_dead = Arc::clone(&self.sidecar_dead);
        let pcm_epoch = Arc::clone(&self.sidecar_epoch);
        std::thread::spawn(move || drain_pcm(stdout, pcm_state, pcm_dead, pcm_epoch, epoch));
        let event_state = Arc::clone(&self.state);
        let event_dead = Arc::clone(&self.sidecar_dead);
        let event_epoch = Arc::clone(&self.sidecar_epoch);
        std::thread::spawn(move || {
            drain_events(stderr, event_state, event_dead, event_epoch, epoch)
        });
        *self.child_lock() = Some(child);
        Ok(())
    }

    fn stop_child(&mut self) {
        self.sidecar_epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(mut child) = self.child_lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn refresh_sidecar_exit(&self) {
        if let Some(child) = self.child_lock().as_mut() {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => self.sidecar_dead.store(true, Ordering::SeqCst),
                Ok(None) => {}
            }
        }
    }

    fn child_lock(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn mark_sidecar_exited(&self) {
        self.sidecar_dead.store(true, Ordering::SeqCst);
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop_child();
    }
}

fn hide_windows_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// AGC 目标峰值（约 40% 满量程）与增益限幅。实测麦克风电平仅 ~4% FS，
/// Silero/SmartTurn/服务端 ASR 在低电平下都不稳定（碎裂/漏检）。
const AGC_TARGET_PEAK: f32 = 0.4 * 32768.0;
const AGC_GAIN_MIN: f32 = 1.0;
const AGC_GAIN_MAX: f32 = 8.0;

/// 对 PCM16 单声道按增益放大并限幅；返回 None 表示增益为 1 无需拷贝。
fn apply_gain(pcm: &[u8], gain: f32) -> Option<Vec<u8>> {
    if gain <= 1.0 + f32::EPSILON {
        return None;
    }
    Some(
        pcm.chunks_exact(2)
            .flat_map(|chunk| {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 * gain;
                (sample.clamp(-32768.0, 32767.0) as i16).to_le_bytes()
            })
            .collect::<Vec<u8>>(),
    )
}

/// 热路径公共体：ring、回声窗、打断监听、分段器、tap 上行。IPC 推流与
/// sidecar 采集线程共用同一条路，保证两种输入源行为一致。
fn ingest_into_state(state: &mut CaptureState, pcm: &[u8]) {
    state.ring.push(pcm);
    // AGC：按块峰值自适应增益（平滑防跳变）。进分段器与 barge 打断监听。
    let chunk_peak = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]).unsigned_abs())
        .max()
        .unwrap_or(0) as f32;
    if chunk_peak > 0.0 {
        let target = (AGC_TARGET_PEAK / chunk_peak).clamp(AGC_GAIN_MIN, AGC_GAIN_MAX);
        state.agc_gain += (target - state.agc_gain) * 0.25;
    }
    let gained;
    let pcm_for_vad: &[u8] = match apply_gain(pcm, state.agc_gain) {
        Some(gained_pcm) => {
            gained = gained_pcm;
            &gained
        }
        None => pcm,
    };
    let suppressed = state.echo_until.is_some_and(|until| Instant::now() < until);
    // 经 guard 的字段投影借用不相交不成立（deref_mut 独占整个 guard），先拆出可变借用。
    let CaptureState {
        barge_in,
        barge_flag,
        barge_utterance,
        tap,
        tap_dropped,
        segmenter,
        ..
    } = &mut *state;
    // barge 监听与分段器同吃 AGC 后信号：实测麦克风电平仅 ~4% FS，原始电平
    // 喂 Silero 达不到 0.5 概率阈值，播报打断永不触发（打不断的主要根因）。
    // 放大后的回声理论上更易误触发打断，兜底是泵侧只在 turn.responding 时
    // clear+cancel——误打断的代价（AI 停一下重说）远小于打不断。
    if suppressed
        && let Some(monitor) = barge_in.as_mut()
        && !barge_flag.load(Ordering::SeqCst)
    {
        monitor.ingest(pcm_for_vad);
        if monitor.triggered() {
            *barge_utterance = monitor.take_staged();
            barge_flag.store(true, Ordering::SeqCst);
        }
    }
    segmenter.ingest(pcm_for_vad, suppressed);
    if let Some(tap) = tap.as_ref()
        && tap.try_send(pcm_for_vad.to_vec()).is_err()
    {
        tap_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

fn mark_sidecar_dead(dead: &AtomicBool, epoch: &AtomicU64, mine: u64) {
    if epoch.load(Ordering::SeqCst) == mine {
        dead.store(true, Ordering::SeqCst);
    }
}

fn drain_pcm(
    mut stdout: impl Read,
    state: Arc<Mutex<CaptureState>>,
    dead: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    mine: u64,
) {
    let mut buf = [0_u8; 8192];
    loop {
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => {
                mark_sidecar_dead(&dead, &epoch, mine);
                break;
            }
            Ok(n) => {
                {
                    let mut state = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if epoch.load(Ordering::SeqCst) != mine {
                        break;
                    }
                    ingest_into_state(&mut state, &buf[..n]);
                }
                drain_realtime_signals(&state);
            }
        }
    }
}

fn drain_events(
    stderr: impl Read,
    state: Arc<Mutex<CaptureState>>,
    dead: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    mine: u64,
) {
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if let Some(peak) = parse_level_peak(&line) {
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if epoch.load(Ordering::SeqCst) != mine {
                break;
            }
            state.last_peak = peak;
        }
    }
    mark_sidecar_dead(&dead, &epoch, mine);
}

#[cfg(test)]
mod tests {
    use super::{
        AudioCapture, AudioError, NoopSink, PlaybackSink, RealtimeCaptureSignal, RecordingSink,
        SidecarPoll, bridge_command_args, parse_level_peak,
    };
    use crate::audio::pcm::RING_CAPACITY_BYTES;
    use crate::processes::{FailingProcessEnumerator, InjectedProcessEnumerator, MeetingProcess};
    use std::path::PathBuf;

    fn meeting(pid: u32, name: &str, title: &str) -> MeetingProcess {
        MeetingProcess {
            pid,
            name: name.to_string(),
            title: title.to_string(),
        }
    }

    fn allowed_zoom(pid: u32) -> InjectedProcessEnumerator {
        InjectedProcessEnumerator::new(vec![meeting(pid, "zoom.exe", "Standup")])
    }

    fn le_i16(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn from_injected_stores_pcm_in_memory_ring() {
        let mut capture = AudioCapture::from_injected();
        let pcm = le_i16(&[100, 200, 300]);
        capture.push_pcm(&pcm);
        assert_eq!(capture.snapshot_48k(), pcm);
        assert_eq!(capture.overrun_count(), 0);
        assert_eq!(capture.pcm_for_asr(), le_i16(&[200]));
    }

    /// 实时信号：本地分段器判定说完后 push_pcm 应发出 CommitTurn；
    /// 未注册 sink（级联路线）时不发。分段触发依赖能量实现，与工厂用例
    /// 共用锁串行执行。
    #[test]
    fn push_pcm_emits_commit_turn_signal_when_segment_ready() {
        crate::audio::segmenter::factory_test_support::with_vad_off(|| {
            let mut capture = AudioCapture::from_injected();
            let (tx, rx) = std::sync::mpsc::channel();
            capture.set_realtime_sink(Some(tx));

            // 240ms 语音 + 800ms 静音：超过 START/MIN_SPEECH/END_SILENCE 阈值。
            let mut pcm = voiced_segment();
            pcm.extend_from_slice(&silence_for(800));
            capture.push_pcm(&pcm);

            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_secs(2)),
                Ok(RealtimeCaptureSignal::CommitTurn)
            );
        });
    }

    /// 打断链路（AGC 馈电核心用例）：实测麦克风电平 ~4% FS（约 1300/32768），
    /// 原始电平喂 Silero 达不到 0.5 阈值；经 AGC 增益放大后必须能触发打断。
    /// VAD 替身按「窗口峰值严格超过原始块峰值」判定人声：原始信号增益为 1,
    /// 峰值恒等于 raw_peak 永远不触发；只有 AGC 增益后的信号（>raw_peak）
    /// 才会被判人声——钉住监听器吃的是增益后信号而非原始信号。
    #[test]
    fn barge_monitor_ingests_agc_gained_signal_not_raw() {
        struct GainAwareVad {
            raw_peak: i16,
        }
        impl crate::audio::vad::VoiceActivityDetector for GainAwareVad {
            fn process(&mut self, window: &[f32]) -> f32 {
                let peak = (window
                    .iter()
                    .fold(0.0f32, |max, sample| max.max(sample.abs()))
                    * 32768.0) as i32;
                if peak > i32::from(self.raw_peak) + 64 {
                    0.9
                } else {
                    0.0
                }
            }
        }

        let mut capture = AudioCapture::from_injected();
        // 小电平人声：峰值约 4% FS，连续帧（每帧 20ms@48k = 960 采样）。
        let quiet_voice = le_i16(&[1300i16; 960]);
        capture.stage_barge_monitor_for_tests(Box::new(GainAwareVad { raw_peak: 1300 }));
        // 进入回声抑制窗（播报期间），barge 监听只在此窗工作。
        capture.suppress_echo_for(std::time::Duration::from_secs(30));

        for _ in 0..14 {
            capture.push_pcm(&quiet_voice);
        }
        assert!(
            capture
                .barge_in_flag()
                .load(std::sync::atomic::Ordering::SeqCst),
            "AGC-gained quiet voice must trigger barge-in (raw level alone cannot)"
        );
        let utterance = capture.take_barge_in_utterance().expect("staged utterance");
        assert!(
            utterance.len() >= 8 * 3072,
            "utterance should cover the triggered windows (gained PCM)"
        );
    }

    /// 打断信号：播报抑制窗内监听器触发（暂存话音）→ BargeIn 信号，
    /// 且旗标复位允许后续再次触发。触发源用注入替身，不依赖 Silero 模型。
    #[test]
    fn push_pcm_emits_barge_in_signal_and_resets_flag() {
        let mut capture = AudioCapture::from_injected();
        let (tx, rx) = std::sync::mpsc::channel();
        capture.set_realtime_sink(Some(tx));

        capture.stage_barge_utterance_for_tests(vec![1, 2, 3, 4]);
        capture
            .barge_in_flag()
            .store(true, std::sync::atomic::Ordering::SeqCst);
        capture.push_pcm(&[0u8; 8]);

        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(RealtimeCaptureSignal::BargeIn)
        );
        assert!(
            !capture
                .barge_in_flag()
                .load(std::sync::atomic::Ordering::SeqCst),
            "flag must reset so the monitor can trigger again"
        );
        // 暂存话音已消费，不落入 ASR 待取队列。
        assert!(!capture.utterance_ready());
    }

    fn voiced_segment() -> Vec<u8> {
        let mut pcm = Vec::new();
        for _ in 0..12 {
            // 20ms@48k 一帧，幅值远超能量阈值。
            pcm.extend_from_slice(&le_i16(&[9000i16; 960]));
        }
        pcm
    }

    fn silence_for(millis: u64) -> Vec<u8> {
        vec![0u8; (48 * 2 * millis) as usize]
    }

    #[test]
    fn retired_sidecar_cannot_write_pcm_levels_or_exit_state() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        };
        let capture = AudioCapture::from_injected();
        let dead = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(2));
        super::drain_pcm(
            &[1_u8, 2, 3, 4][..],
            Arc::clone(&capture.state),
            Arc::clone(&dead),
            Arc::clone(&epoch),
            1,
        );
        super::drain_events(
            &b"{\"type\":\"level\",\"peak\":0.9}\n"[..],
            Arc::clone(&capture.state),
            Arc::clone(&dead),
            Arc::clone(&epoch),
            1,
        );
        assert!(capture.snapshot_48k().is_empty());
        assert_eq!(capture.last_peak(), 0.0);
        assert!(!dead.load(Ordering::SeqCst));
    }

    #[test]
    fn injected_overrun_drops_oldest() {
        let mut capture = AudioCapture::from_injected();
        capture.push_pcm(&vec![0x10; RING_CAPACITY_BYTES]);
        capture.push_pcm(&[0xFE, 0xFF]);
        assert_eq!(capture.overrun_count(), 1);
        let snap = capture.snapshot_48k();
        assert_eq!(snap.len(), RING_CAPACITY_BYTES);
        assert_eq!(&snap[RING_CAPACITY_BYTES - 2..], &[0xFE, 0xFF]);
    }

    #[test]
    fn parses_level_json_and_ignores_other_events() {
        let mut capture = AudioCapture::from_injected();
        capture.ingest_event_line(r#"{"type":"level","sequence":0,"peak":0.42}"#);
        assert!((capture.last_peak() - 0.42).abs() < 1e-9);
        capture.ingest_event_line(r#"{"type":"ready","sequence":1,"captureScope":"process-tree"}"#);
        assert!((capture.last_peak() - 0.42).abs() < 1e-9);
        assert_eq!(
            parse_level_peak(r#"{"type":"level","sequence":2,"peak":0.75}"#),
            Some(0.75)
        );
        assert_eq!(
            parse_level_peak(r#"{"type":"process-exited","sequence":3}"#),
            None
        );
    }

    #[test]
    fn restart_once_then_second_crash_fails() {
        let mut capture = AudioCapture::from_injected();
        capture
            .restart_once()
            .expect("first sidecar restart is allowed");
        let error = capture
            .restart_once()
            .expect_err("second crash is terminal");
        assert_eq!(error, AudioError::SidecarFailed);
        assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
    }

    #[test]
    fn poll_sidecar_exit_allows_restart_once_then_fails() {
        let mut capture = AudioCapture::from_injected();
        assert_eq!(
            capture.poll_sidecar().expect("injected starts alive"),
            SidecarPoll::Alive
        );
        capture.mark_sidecar_exited();
        assert_eq!(
            capture.poll_sidecar().expect("injected exit is visible"),
            SidecarPoll::Exited
        );
        capture
            .restart_once()
            .expect("first sidecar restart is allowed");
        assert_eq!(
            capture.poll_sidecar().expect("restart clears exit"),
            SidecarPoll::Alive
        );
        capture.mark_sidecar_exited();
        assert_eq!(
            capture.poll_sidecar().expect("second crash is visible"),
            SidecarPoll::Exited
        );
        let error = capture
            .restart_once()
            .expect_err("second crash is terminal");
        assert_eq!(error, AudioError::SidecarFailed);
        assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
    }

    #[test]
    fn spawn_bridge_skips_when_exe_absent() {
        let missing = PathBuf::from("definitely-missing-AudioBridge.exe");
        assert!(!missing.is_file());
        let enumerator = allowed_zoom(4242);
        let error =
            AudioCapture::spawn_bridge(&missing, 4242, &enumerator).expect_err("missing exe");
        assert_eq!(error, AudioError::ExeMissing);
        assert_eq!(error.code(), "SESSION_SIDECAR_MISSING");
        assert!(enumerator.call_count() >= 1);
    }

    #[test]
    fn spawn_bridge_rejects_pid_zero() {
        let missing = PathBuf::from("definitely-missing-AudioBridge.exe");
        let error = AudioCapture::spawn_bridge(&missing, 0, &allowed_zoom(1)).expect_err("pid 0");
        assert_eq!(error, AudioError::InvalidPid);
    }

    #[test]
    fn spawn_bridge_rejects_pid_not_on_fresh_allowlist() {
        let missing = PathBuf::from("definitely-missing-AudioBridge.exe");
        let enumerator = allowed_zoom(100);
        let listed = crate::processes::list_meeting_processes(&enumerator).expect("listed");
        assert!(listed.iter().any(|process| process.pid == 100));
        enumerator.set(Vec::new());
        let error = AudioCapture::spawn_bridge(&missing, 100, &enumerator)
            .expect_err("stale pid must be rejected");
        assert_eq!(error, AudioError::ProcessNotAvailable);
        assert_eq!(error.code(), "MEETING_PROCESS_NOT_AVAILABLE");
        assert!(enumerator.call_count() >= 2);
    }

    #[test]
    fn spawn_bridge_rejects_unknown_exe_even_when_title_present() {
        let missing = PathBuf::from("definitely-missing-AudioBridge.exe");
        let enumerator = InjectedProcessEnumerator::new(vec![meeting(77, "notepad.exe", "Notes")]);
        let error = AudioCapture::spawn_bridge(&missing, 77, &enumerator)
            .expect_err("notepad is not allowlisted");
        assert_eq!(error, AudioError::ProcessNotAvailable);
    }

    #[test]
    fn spawn_bridge_fails_closed_when_enumerator_errors() {
        let missing = PathBuf::from("definitely-missing-AudioBridge.exe");
        let error = AudioCapture::spawn_bridge(&missing, 100, &FailingProcessEnumerator)
            .expect_err("enumeration failure");
        assert_eq!(error, AudioError::SpawnFailed);
        assert_eq!(error.code(), "SESSION_SIDECAR_SPAWN_FAILED");
    }

    #[test]
    fn bridge_args_match_csharp_pid_flag() {
        assert_eq!(bridge_command_args(99), ["--pid", "99"]);
    }

    #[test]
    fn recording_sink_stores_bytes_and_cancel() {
        let mut sink = RecordingSink::default();
        sink.play_pcm(&[0x11, 0x22, 0x33, 0x44], 24_000);
        sink.cancel();
        assert_eq!(sink.recorded(), &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(sink.sample_rate(), Some(24_000));
        assert!(sink.cancelled());
    }

    #[test]
    fn mic_ingest_handle_and_tap_feed_without_sessions_lock() {
        let capture = AudioCapture::from_injected();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
        capture.set_pcm_tap(Some(tx));
        let handle = capture.mic_ingest_handle();
        // 样本取满量程级（≥AGC 目标峰值）：增益被下限钳到 1，tap 应原样转发。
        handle.push_pcm(&le_i16(&[14_000, 16_000]));
        handle.push_pcm(&le_i16(&[13_000, 15_000]));
        assert_eq!(rx.recv().unwrap(), le_i16(&[14_000, 16_000]));
        assert_eq!(rx.recv().unwrap(), le_i16(&[13_000, 15_000]));
        // 容量 2：灌满后继续推不阻塞，丢弃计数。
        handle.push_pcm(&le_i16(&[14_000, 16_000]));
        handle.push_pcm(&le_i16(&[13_000, 15_000]));
        handle.push_pcm(&le_i16(&[14_000, 15_000]));
        assert_eq!(capture.tap_dropped(), 1);
        // 分段器照常收帧（ring/segmenter 不受 tap 影响）。
        // 5 次 push × 4B = 20B。
        assert_eq!(handle.state.lock().unwrap().ring.len(), 20);
        capture.set_pcm_tap(None);
    }

    #[test]
    fn default_sink_is_noop() {
        let mut sink = NoopSink;
        sink.play_pcm(&[0x01, 0x02], 16_000);
        sink.cancel();
    }
}
