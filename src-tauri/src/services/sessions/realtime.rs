//! realtime 子模块：端到端实时泵的装配/拆卸与泵事件取用。
//! 纯搬移自 services/sessions.rs，不含行为变更。
use super::*;

/// 实时泵装配依赖（commands 层在会话启动后构造）。
pub struct RealtimePumpDeps {
    pub endpoint: crate::providers::ProviderEndpoint,
    pub credential: Option<String>,
    pub model_id: String,
    pub voice: String,
    pub instructions: String,
    pub history: Vec<(String, String)>,
    pub auto_respond: bool,
    /// 联网搜索（DashScope 端到端线路 session.update enable_search）。
    pub enable_search: bool,
    pub role_name: String,
    pub hold_playback: bool,
    pub playback_mode: crate::services::realtime_pump::RealtimePlaybackMode,
    pub playback_exe: Option<std::path::PathBuf>,
    pub playback_endpoint_id: Option<String>,
}

impl<S: PlaybackSink> SessionService<S> {
    /// 装配实时会话泵（端到端流式路线）：常驻连接 + 常驻流式播放 + 麦克风 tap。
    /// 幂等：重复调用先关停旧泵（会话重启场景）。
    pub fn attach_realtime_pump(
        &mut self,
        deps: RealtimePumpDeps,
        live_sink: Option<Box<dyn Fn(crate::services::PumpLive) + Send + Sync>>,
    ) -> Result<(), SessionServiceError> {
        self.detach_realtime_pump();
        let (tap_tx, tap_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(50);
        self.capture.set_pcm_tap(Some(tap_tx));
        let playback: std::sync::Arc<dyn crate::services::realtime_pump::PlaybackStream> =
            match deps
                .playback_endpoint_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
            {
                Some(endpoint_id) => {
                    let exe = deps
                        .playback_exe
                        .clone()
                        .ok_or(SessionServiceError::NotFound)?;
                    std::sync::Arc::new(
                        crate::audio::playback::StreamPlayback::start(&exe, endpoint_id, 24_000)
                            .map_err(SessionServiceError::Playback)?,
                    )
                }
                None => std::sync::Arc::new(crate::services::realtime_pump::SilentPlayback),
            };
        let session = crate::providers::realtime_session::RealtimeSession::start(
            crate::providers::realtime_session::RealtimeSessionConfig {
                endpoint: deps.endpoint,
                credential: deps.credential,
                model_id: deps.model_id,
                voice: deps.voice,
                instructions: deps.instructions,
                history: deps.history,
                auto_respond: deps.auto_respond,
                enable_search: deps.enable_search,
            },
        )?;
        let echo_suppress = self.capture.echo_suppress_handle();
        let pump = crate::services::realtime_pump::RealtimePump::start(
            session,
            tap_rx,
            playback,
            crate::services::realtime_pump::PumpConfig {
                auto_respond: deps.auto_respond,
                role_name: deps.role_name,
                hold_playback: deps.hold_playback,
                playback_mode: deps.playback_mode,
                suppress_echo: Some(Box::new(move |duration| {
                    echo_suppress.suppress_for(duration);
                })),
                respond_start_timeout: crate::services::realtime_pump::RESPOND_START_TIMEOUT,
            },
            live_sink,
        );
        self.realtime_shared = Some(std::sync::Arc::clone(&pump.shared));
        self.realtime_pump = Some(pump);
        // 采集侧本地判定（分段说完/打断）→ 泵命令转发桥。Manual 模式
        // （DashScope）服务端 VAD 已禁用，这是转写提交与播报打断的唯一驱动。
        let (signal_tx, signal_rx) =
            std::sync::mpsc::channel::<crate::audio::capture::RealtimeCaptureSignal>();
        self.capture.set_realtime_sink(Some(signal_tx));
        let command_sink = self.realtime_pump.as_ref().map(|pump| pump.command_sink());
        if let Some(command_sink) = command_sink {
            std::thread::Builder::new()
                .name("realtime-signal-bridge".into())
                .spawn(move || {
                    while let Ok(signal) = signal_rx.recv() {
                        let command = match signal {
                            crate::audio::capture::RealtimeCaptureSignal::CommitTurn => {
                                crate::services::realtime_pump::PumpCommand::CommitTurn
                            }
                            crate::audio::capture::RealtimeCaptureSignal::BargeIn => {
                                crate::services::realtime_pump::PumpCommand::LocalBargeIn
                            }
                        };
                        if command_sink.send(command).is_err() {
                            break;
                        }
                    }
                })
                .ok();
        }
        Ok(())
    }

    /// 关停泵并摘除 tap。
    pub fn detach_realtime_pump(&mut self) {
        self.capture.set_realtime_sink(None); // 先断信号源，转发桥随通道关闭退出
        self.realtime_pump.take(); // Drop → Shutdown + join
        self.realtime_shared.take();
        self.capture.set_pcm_tap(None);
    }

    /// 取走最旧的完成轮（finalize 持久化用）。
    pub fn take_realtime_turn(&self) -> Option<crate::services::realtime_pump::CompletedTurn> {
        self.realtime_shared
            .as_ref()
            .and_then(|shared| shared.take_completed())
    }

    /// 泵是否在跑（端到端流式路线装配成功）。
    pub fn realtime_pump_running(&self) -> bool {
        self.realtime_pump.is_some()
    }

    /// 是否存在可强制回答的仅转写发言（热键/按钮的 NOTHING_TO_ANSWER 判定）。
    pub fn realtime_has_transcript_only(&self) -> bool {
        self.realtime_shared.as_ref().is_some_and(|shared| {
            shared
                .last_transcript_only
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
        })
    }

    /// 强制会议助手回答最近一条仅转写发言（Ctrl+Alt+A / 「让助手回答」）。
    /// 回答由泵异步生成，前端就绪轮询落库；返回 false 表示无泵（级联路线
    /// 或未装配），调用方应回退级联 forced 分支。
    pub fn trigger_realtime_assistant(&self) -> bool {
        let Some(pump) = self.realtime_pump.as_ref() else {
            return false;
        };
        pump.send(crate::services::realtime_pump::PumpCommand::ForceRespond);
        true
    }

    /// 取走泵的终局错误（一次）。
    pub fn take_realtime_failure(&self) -> Option<String> {
        let shared = self.realtime_shared.as_ref()?;
        let mut slot = shared.failed.lock().unwrap_or_else(|p| p.into_inner());
        slot.take()
    }

    /// 每轮落库后同步上下文（重连时回放最近 N 轮）。
    pub fn push_realtime_history(&self, history: Vec<(String, String)>) {
        if let Some(pump) = self.realtime_pump.as_ref() {
            pump.send(crate::services::realtime_pump::PumpCommand::SetHistory(
                history,
            ));
        }
    }

    /// 视频帧（摄像头/桌面共享，base64 JPEG）直通实时会话。
    /// 返回 false 表示当前无泵会话（级联/未开始/降级旧路径），前端应停止推帧。
    pub fn push_video_frame(&self, jpeg_b64: &str) -> bool {
        self.realtime_pump
            .as_ref()
            .map(|pump| {
                pump.send(crate::services::realtime_pump::PumpCommand::AppendImage(
                    jpeg_b64.to_owned(),
                ));
                true
            })
            .unwrap_or(false)
    }

    /// 候选确认：放行扣住的音频。
    pub fn flush_realtime_held(&self) {
        if let Some(pump) = self.realtime_pump.as_ref() {
            pump.send(crate::services::realtime_pump::PumpCommand::FlushHeld);
        }
    }

    /// 候选拒绝/接管：丢弃扣住的音频。
    pub fn discard_realtime_held(&self) {
        if let Some(pump) = self.realtime_pump.as_ref() {
            pump.send(crate::services::realtime_pump::PumpCommand::DiscardHeld);
        }
    }
}
