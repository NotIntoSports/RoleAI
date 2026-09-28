//! Bounded PCM playback through the existing, application-owned AudioBridge.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub struct AudioOutputDevice {
    pub id: String,
    pub name: String,
}

pub fn list_outputs(executable: &std::path::Path) -> Result<Vec<AudioOutputDevice>, &'static str> {
    use std::io::Read;
    if !executable.is_file() {
        return Err("SESSION_SIDECAR_MISSING");
    }
    let mut command = Command::new(executable);
    command
        .arg("--list-output-devices")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|_| "OUTPUT_ENUMERATION_FAILED")?;
    let output = child.stdout.take().ok_or("OUTPUT_ENUMERATION_FAILED")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output.take(65537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let started = Instant::now();
    let success = loop {
        if started.elapsed() > Duration::from_secs(5) {
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Err(_) => break false,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    if !success {
        let _ = child.kill();
    }
    let _ = child.wait();
    let bytes = reader
        .join()
        .ok()
        .and_then(Result::ok)
        .ok_or("OUTPUT_ENUMERATION_FAILED")?;
    if !success || bytes.len() > 65536 {
        return Err("OUTPUT_ENUMERATION_FAILED");
    }
    serde_json::from_slice(&bytes).map_err(|_| "OUTPUT_ENUMERATION_FAILED")
}

#[derive(Debug, Clone)]
pub struct BridgePlayback {
    pub executable: PathBuf,
    pub endpoint_id: String,
}

impl BridgePlayback {
    pub fn play(
        &self,
        pcm: &[u8],
        sample_rate: u32,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        if cancelled() {
            return Err("PLAYBACK_CANCELLED");
        }
        if pcm.is_empty()
            || !pcm.len().is_multiple_of(2)
            || pcm.len() > 16 * 1024 * 1024
            || !(8000..=48000).contains(&sample_rate)
            || self.endpoint_id.trim().is_empty()
        {
            return Err("PLAYBACK_FORMAT_INVALID");
        }
        if !self.executable.is_file() {
            return Err("SESSION_SIDECAR_MISSING");
        }
        let mut command = Command::new(&self.executable);
        command
            .arg("--play-pcm")
            .arg(&self.endpoint_id)
            .arg(sample_rate.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| "PLAYBACK_START_FAILED")?;
        let completion = child
            .stdout
            .take()
            .map(|pipe| crate::prerequisites::read_pipe_bounded(pipe, 128, None));
        let Some(mut input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("PLAYBACK_START_FAILED");
        };
        let payload = pcm.to_vec();
        let writer = std::thread::spawn(move || input.write_all(&payload));
        let started = Instant::now();
        let budget = Duration::from_secs_f64(pcm.len() as f64 / (sample_rate as f64 * 2.0) + 15.0);
        let result = loop {
            if cancelled() {
                break Err("PLAYBACK_CANCELLED");
            }
            if started.elapsed() > budget {
                break Err("PLAYBACK_TIMEOUT");
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    break if status.success() {
                        Ok(())
                    } else {
                        Err("PLAYBACK_FAILED")
                    };
                }
                Err(_) => break Err("PLAYBACK_FAILED"),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let written = writer.join().ok().and_then(Result::ok).is_some();
        if cancelled() {
            return Err("PLAYBACK_CANCELLED");
        }
        result.and(if written {
            completion
                .and_then(|reader| reader.recv_timeout(Duration::from_secs(1)).ok())
                .ok_or("PLAYBACK_NOT_CONFIRMED")
                .and_then(|output| verify_playback_completion(&output))
        } else {
            Err("PLAYBACK_WRITE_FAILED")
        })
    }
}

fn verify_playback_completion(output: &[u8]) -> Result<(), &'static str> {
    if output.len() <= 128
        && std::str::from_utf8(output).is_ok_and(|text| text.trim() == "PLAYBACK_COMPLETED")
    {
        Ok(())
    } else {
        Err("PLAYBACK_NOT_CONFIRMED")
    }
}

/// 流式播放帧协议（与 AudioBridge `--play-stream` 对齐）：
/// `[u32 kind LE][u32 len LE][payload]`；kind 1=audio 2=clear 3=drain 4=ping。
pub const STREAM_FRAME_KIND_AUDIO: u32 = 1;
pub const STREAM_FRAME_KIND_CLEAR: u32 = 2;
pub const STREAM_FRAME_KIND_DRAIN: u32 = 3;
pub const STREAM_FRAME_KIND_PING: u32 = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackDiagnostics {
    pub generation: u64,
    pub restarts: u64,
    pub last_event: String,
}

pub fn encode_stream_frame(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(8 + payload.len());
    frame.extend_from_slice(&kind.to_le_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[derive(Clone, Debug)]
enum StreamCommand {
    Audio(Vec<u8>),
    Clear,
    Drain,
    Ping,
}

/// 常驻流式播放句柄：会话期间拉起一次 AudioBridge `--play-stream`，
/// 音频/打断/播净经通道送写入线程编码成帧；崩溃允许重启一次。
pub struct StreamPlayback {
    executable: PathBuf,
    endpoint_id: String,
    sample_rate: u32,
    sender: Mutex<Option<mpsc::Sender<StreamCommand>>>,
    child: Mutex<Option<Child>>,
    sidecar_dead: Arc<AtomicBool>,
    /// 每次 spawn 递增；旧进程的 stdout 线程不能把新进程标记为死亡。
    sidecar_generation: Arc<AtomicU64>,
    restart_count: AtomicU64,
    last_event: Arc<Mutex<String>>,
    /// sidecar 已播净当前缓冲（`playback.state=drained` 回执，消费型）。
    stream_drained: Arc<AtomicBool>,
    restart_lock: Mutex<()>,
    last_restart: Mutex<Option<Instant>>,
}

impl StreamPlayback {
    pub fn start(
        executable: &Path,
        endpoint_id: &str,
        sample_rate: u32,
    ) -> Result<Self, &'static str> {
        if !(8000..=48000).contains(&sample_rate) || endpoint_id.trim().is_empty() {
            return Err("PLAYBACK_FORMAT_INVALID");
        }
        if !executable.is_file() {
            return Err("SESSION_SIDECAR_MISSING");
        }
        let playback = Self {
            executable: executable.to_path_buf(),
            endpoint_id: endpoint_id.to_owned(),
            sample_rate,
            sender: Mutex::new(None),
            child: Mutex::new(None),
            sidecar_dead: Arc::new(AtomicBool::new(false)),
            sidecar_generation: Arc::new(AtomicU64::new(0)),
            restart_count: AtomicU64::new(0),
            last_event: Arc::new(Mutex::new(String::new())),
            stream_drained: Arc::new(AtomicBool::new(true)),
            restart_lock: Mutex::new(()),
            last_restart: Mutex::new(None),
        };
        playback.spawn_worker()?;
        Ok(playback)
    }

    /// 音频帧（16-bit 单声道 PCM）。非阻塞投递，编码与写盘都在写入线程。
    pub fn write(&self, pcm: &[u8]) -> Result<(), &'static str> {
        self.send(StreamCommand::Audio(pcm.to_vec()))
    }

    /// 打断：清空未播缓冲，出声在设备延迟内停止。
    pub fn clear(&self) -> Result<(), &'static str> {
        self.send(StreamCommand::Clear)
    }

    /// 播净当前缓冲后由 sidecar 回执 PLAYBACK_DRAINED（诊断/候选冲刷用）。
    pub fn drain(&self) -> Result<(), &'static str> {
        // 先清残留标记再发 drain：只认本次 drain 之后到达的回执。
        self.stream_drained.store(false, Ordering::SeqCst);
        self.send(StreamCommand::Drain)
    }

    /// 无副作用保活帧；用于 WebAudio 主路径下持续验证原生兜底进程可用。
    pub fn ping(&self) -> Result<(), &'static str> {
        self.send(StreamCommand::Ping)
    }

    /// 取走「已播净」标记（消费型）：实时泵的回声闸门据此重新开放上行。
    pub fn take_stream_drained(&self) -> bool {
        self.stream_drained.swap(false, Ordering::SeqCst)
    }

    pub fn is_alive(&self) -> bool {
        !self.sidecar_dead.load(Ordering::SeqCst)
    }

    pub fn diagnostics(&self) -> PlaybackDiagnostics {
        PlaybackDiagnostics {
            generation: self.sidecar_generation.load(Ordering::SeqCst),
            restarts: self.restart_count.load(Ordering::SeqCst),
            last_event: self
                .last_event
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    /// 播放 sidecar 是会话级基础设施，退出后按 500ms 冷却自动重建。
    /// 旧 stdout 线程带代际检查，不会覆盖新一代进程的存活状态。
    fn restart_dead_worker(&self) -> Result<(), &'static str> {
        let _guard = self.restart_lock.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mut last = self.last_restart.lock().unwrap_or_else(|p| p.into_inner());
            if last.is_some_and(|at| at.elapsed() < Duration::from_millis(500)) {
                return Err("PLAYBACK_START_FAILED");
            }
            *last = Some(Instant::now());
        }
        self.restart_count.fetch_add(1, Ordering::SeqCst);
        self.stop_worker();
        self.spawn_worker()
    }

    fn send(&self, command: StreamCommand) -> Result<(), &'static str> {
        // 常驻播放进程偶发退出/管道写失败后，sender 仍可能短暂可用。这里在
        // 事件线程标记死亡后自动重建一次，避免后续轮次文本正常但永远无声。
        if self.sidecar_dead.load(Ordering::SeqCst) {
            self.restart_dead_worker()?;
        }
        if let Err(command) = self.try_send(command) {
            // receiver 已断通常意味着写入线程/管道先于 stdout 线程退出；立即
            // 重建，不等下一次 delta 才恢复。
            self.restart_dead_worker()?;
            return self.try_send(command).map_err(|_| "PLAYBACK_START_FAILED");
        }
        Ok(())
    }

    fn try_send(&self, command: StreamCommand) -> Result<(), StreamCommand> {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match sender.as_ref() {
            Some(sender) => sender.send(command).map_err(|error| error.0),
            None => Err(command),
        }
    }

    fn spawn_worker(&self) -> Result<(), &'static str> {
        let generation = self.sidecar_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.sidecar_dead.store(false, Ordering::SeqCst);
        let mut command = Command::new(&self.executable);
        command
            .arg("--play-stream")
            .arg(&self.endpoint_id)
            .arg(self.sample_rate.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| "PLAYBACK_START_FAILED")?;
        let stdin = child.stdin.take().ok_or("PLAYBACK_START_FAILED")?;
        let stdout = child.stdout.take().ok_or("PLAYBACK_START_FAILED")?;
        let (tx, rx) = mpsc::channel::<StreamCommand>();
        std::thread::spawn(move || pump_stream_commands(rx, stdin));
        let dead = Arc::clone(&self.sidecar_dead);
        let drained = Arc::clone(&self.stream_drained);
        let generations = Arc::clone(&self.sidecar_generation);
        let last_event = Arc::clone(&self.last_event);
        *last_event.lock().unwrap_or_else(|p| p.into_inner()) = "starting".to_owned();
        std::thread::spawn(move || {
            watch_stream_events(stdout, dead, drained, generation, generations, last_event)
        });
        *self.sender.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
        *self.child.lock().unwrap_or_else(|p| p.into_inner()) = Some(child);
        Ok(())
    }

    fn stop_worker(&self) {
        *self.sender.lock().unwrap_or_else(|p| p.into_inner()) = None;
        if let Some(mut child) = self.child.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for StreamPlayback {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

/// 写入线程：把命令编码成帧并写入 sidecar stdin；通道关闭或写失败即退出。
fn pump_stream_commands(rx: mpsc::Receiver<StreamCommand>, mut stdin: impl std::io::Write) {
    while let Ok(command) = rx.recv() {
        let frame = match command {
            StreamCommand::Audio(pcm) => encode_stream_frame(STREAM_FRAME_KIND_AUDIO, &pcm),
            StreamCommand::Clear => encode_stream_frame(STREAM_FRAME_KIND_CLEAR, &[]),
            StreamCommand::Drain => encode_stream_frame(STREAM_FRAME_KIND_DRAIN, &[]),
            StreamCommand::Ping => encode_stream_frame(STREAM_FRAME_KIND_PING, &[]),
        };
        if stdin.write_all(&frame).is_err() || stdin.flush().is_err() {
            return;
        }
    }
}

/// 事件线程：监听 sidecar stdout；stdout 关闭即视为 sidecar 退出（死标记）。
/// `playback.state=drained` 回执置已播净标记（实时泵回声闸门消费）。
fn watch_stream_events(
    stdout: impl std::io::Read,
    dead: Arc<AtomicBool>,
    drained: Arc<AtomicBool>,
    generation: u64,
    generations: Arc<AtomicU64>,
    last_event: Arc<Mutex<String>>,
) {
    use std::io::{BufRead, BufReader};
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let current = generations.load(Ordering::SeqCst) == generation;
        if current
            && let Some(state) = line.split("\"state\":\"").nth(1)
            && let Some(state) = state.split('"').next()
        {
            *last_event.lock().unwrap_or_else(|p| p.into_inner()) = state.to_owned();
        }
        if line.contains("\"state\":\"drained\"") {
            if current {
                drained.store(true, Ordering::SeqCst);
            }
        } else if line.contains("\"state\":\"failed\"") {
            tracing::warn!("play-stream sidecar failed");
        }
        tracing::debug!(line, "play-stream event");
    }
    if generations.load(Ordering::SeqCst) == generation {
        dead.store(true, Ordering::SeqCst);
        *last_event.lock().unwrap_or_else(|p| p.into_inner()) = "stdout_closed".to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clean_exit_without_completion_marker_is_not_playback_success() {
        assert_eq!(
            verify_playback_completion(b""),
            Err("PLAYBACK_NOT_CONFIRMED")
        );
        assert_eq!(
            verify_playback_completion(b"not completed"),
            Err("PLAYBACK_NOT_CONFIRMED")
        );
        assert_eq!(
            verify_playback_completion(b"PLAYBACK_COMPLETED\r\n"),
            Ok(())
        );
    }
    #[test]
    fn rejects_cancelled_invalid_or_missing_output_before_spawning() {
        let output = BridgePlayback {
            executable: PathBuf::from("missing-playback.exe"),
            endpoint_id: "test-device".into(),
        };
        assert_eq!(
            output.play(&[1, 2], 24000, || true),
            Err("PLAYBACK_CANCELLED")
        );
        assert_eq!(
            output.play(&[1], 24000, || false),
            Err("PLAYBACK_FORMAT_INVALID")
        );
        assert_eq!(
            output.play(&[1, 2], 0, || false),
            Err("PLAYBACK_FORMAT_INVALID")
        );
        assert_eq!(
            output.play(&[1, 2], 24000, || false),
            Err("SESSION_SIDECAR_MISSING")
        );
    }

    #[test]
    fn stream_frame_encoding_matches_sidecar_protocol() {
        let frame = encode_stream_frame(STREAM_FRAME_KIND_AUDIO, &[0xAB, 0xCD]);
        assert_eq!(
            frame,
            vec![1, 0, 0, 0, 2, 0, 0, 0, 0xAB, 0xCD],
            "[kind LE][len LE][payload]"
        );
        assert_eq!(
            encode_stream_frame(STREAM_FRAME_KIND_CLEAR, &[]),
            vec![2, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode_stream_frame(STREAM_FRAME_KIND_DRAIN, &[]),
            vec![3, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    /// 意义：写入线程必须把命令流无损转成帧序列——打断清空帧与音频帧的相对顺序
    /// 错乱会把上一轮的尾巴留在设备里（打断延迟直接劣化）。
    #[test]
    fn command_pump_encodes_commands_in_order_and_stops_on_write_error() {
        struct BrokenPipe;
        impl std::io::Write for BrokenPipe {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let (tx, rx) = mpsc::channel();
        tx.send(StreamCommand::Audio(vec![1, 2, 3, 4])).unwrap();
        tx.send(StreamCommand::Clear).unwrap();
        tx.send(StreamCommand::Drain).unwrap();
        drop(tx);
        let mut sink = Vec::new();
        pump_stream_commands(rx, &mut sink);
        assert_eq!(
            sink,
            [
                encode_stream_frame(STREAM_FRAME_KIND_AUDIO, &[1, 2, 3, 4]),
                encode_stream_frame(STREAM_FRAME_KIND_CLEAR, &[]),
                encode_stream_frame(STREAM_FRAME_KIND_DRAIN, &[]),
            ]
            .concat()
        );

        // 写失败后必须退出，不得吞帧继续。
        let (tx2, rx2) = mpsc::channel();
        tx2.send(StreamCommand::Clear).unwrap();
        tx2.send(StreamCommand::Drain).unwrap();
        drop(tx2);
        pump_stream_commands(rx2, &mut BrokenPipe);
    }

    #[test]
    fn stream_playback_rejects_invalid_config_or_missing_sidecar() {
        let missing = Path::new("definitely-missing-play-stream.exe");
        assert!(
            matches!(
                StreamPlayback::start(missing, "dev", 24_000),
                Err("SESSION_SIDECAR_MISSING")
            ),
            "缺失 sidecar 必须拒绝"
        );
        let existing = std::env::temp_dir().join("roleai-empty-play-stream.exe");
        std::fs::write(&existing, b"stub").unwrap();
        assert!(matches!(
            StreamPlayback::start(&existing, "  ", 24_000),
            Err("PLAYBACK_FORMAT_INVALID")
        ));
        assert!(matches!(
            StreamPlayback::start(&existing, "dev", 7_000),
            Err("PLAYBACK_FORMAT_INVALID")
        ));
        assert!(matches!(
            StreamPlayback::start(&existing, "dev", 49_000),
            Err("PLAYBACK_FORMAT_INVALID")
        ));
        std::fs::remove_file(&existing).ok();
    }
}
