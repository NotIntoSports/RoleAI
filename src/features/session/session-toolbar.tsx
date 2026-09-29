import { ChevronDown, Wrench } from "lucide-react";

import type {
  AudioOutputDevice,
  MeetingProcess,
  PublicConfig,
  RoleScenario,
  VirtualAudioPreparation,
} from "../../generated/bindings";

const PHASE_LABELS: Record<string, string> = {
  idle: "未开始",
  preparing: "准备中",
  listening: "聆听中",
  thinking: "思考中",
  speaking: "回复中",
  stopping: "停止中",
  recovering: "恢复中",
  blocked: "需要处理",
  completed: "已结束",
  failed: "会话异常",
};

const MODE_LABELS: Record<string, string> = {
  ai_active: "AI 应答",
  operator_speaking: "人工接管",
  paused: "已暂停",
  muted: "已静音",
};

const MEETING_NAMES: Record<string, string> = {
  "teams.exe": "Microsoft Teams", "ms-teams.exe": "Microsoft Teams",
  "wemeetapp.exe": "腾讯会议", "feishu.exe": "飞书", "lark.exe": "Lark",
  "dingtalk.exe": "钉钉", "zoom.exe": "Zoom",
};

const PREPARATION_PHASES: Record<string, string> = {
  checking: "检测安装环境", downloading: "下载安装包", verifying: "校验安装包和签名",
  authorizing: "等待 Windows 管理员授权", installing: "安装驱动", rechecking: "重新检测音频端点",
};

interface SessionToolbarProps {
  config: PublicConfig | null;
  busy: boolean;
  active: boolean;
  phase: string;
  mode: string;
  realtimeStatus: string;
  setAllowWebSearch: (allow: boolean) => void;
  inputSource: string;
  setInputSource: (source: string) => void;
  voiceRouteId: string;
  setVoiceRouteId: (id: string) => void;
  configurationOpen: boolean;
  setConfigurationOpen: (open: boolean) => void;
  refreshMeetings: () => Promise<void> | void;
  refreshVirtualAudio: () => Promise<void> | void;
  micActive: boolean;
  meetingPid: string;
  setMeetingPid: (pid: string) => void;
  meetingProcesses: MeetingProcess[];
  selectedRoleScenario: RoleScenario | undefined;
  virtualAudio: VirtualAudioPreparation | null;
  installingAudio: boolean;
  audioRetryBlocked: boolean;
  audioPreparationPhase: string;
  installVirtualAudio: () => Promise<void> | void;
  audioAttempted: boolean;
  outputDeviceId: string;
  setOutputDeviceId: (id: string) => void;
  audioOutputs: AudioOutputDevice[];
  refreshAudioOutputs: () => Promise<void> | void;
  canSearch: boolean;
  allowWebSearch: boolean;
  allowBargeIn: boolean;
  setAllowBargeIn: (allow: boolean) => void;
}

export function SessionToolbar({
  config,
  busy,
  active,
  phase,
  mode,
  realtimeStatus,
  setAllowWebSearch,
  inputSource,
  setInputSource,
  voiceRouteId,
  setVoiceRouteId,
  configurationOpen,
  setConfigurationOpen,
  refreshMeetings,
  refreshVirtualAudio,
  micActive,
  meetingPid,
  setMeetingPid,
  meetingProcesses,
  selectedRoleScenario,
  virtualAudio,
  installingAudio,
  audioRetryBlocked,
  audioPreparationPhase,
  installVirtualAudio,
  audioAttempted,
  outputDeviceId,
  setOutputDeviceId,
  audioOutputs,
  refreshAudioOutputs,
  canSearch,
  allowWebSearch,
  allowBargeIn,
  setAllowBargeIn,
}: SessionToolbarProps) {
  return (
    <header className="session-toolbar">
      <div className="session-toolbar-meta">
        <h2 id="workspace-session-heading">当前会话</h2>
        <span className="status-badge" data-active={active}>
          {PHASE_LABELS[phase] ?? phase}
        </span>
        {realtimeStatus === "reconnecting" && <span className="status-badge" data-active={active}>语音重连中…</span>}
        {realtimeStatus === "failed" && <span className="status-badge" data-active={false}>语音连接失败</span>}
        <span className="session-mode">{MODE_LABELS[mode] ?? mode}</span>
      </div>
      <div className="session-config-heading">
        <span className="session-config-summary">{inputSource === "meeting" ? "会议音频" : "本机麦克风"} · {config?.speech.voiceRoutes.find((route) => route.id === voiceRouteId)?.name ?? "尚未选择语音线路"}</span>
        <button type="button" className="button-ghost" aria-expanded={configurationOpen} aria-controls="session-configuration" onClick={() => setConfigurationOpen(!configurationOpen)}><Wrench size={15} aria-hidden="true" />会话配置<ChevronDown size={14} aria-hidden="true" /></button>
      </div>
      <div id="session-configuration" className="session-configuration" hidden={!configurationOpen}>
        {!config && <p className="muted">尚未读取到会话配置，请到“服务”和“设置”检查线路与角色。</p>}
        {config && <fieldset disabled={busy || active} className="session-selection">
          <legend>本场会话配置</legend>
          <label>输入来源<select value={inputSource} onChange={(event) => { setInputSource(event.target.value); if (event.target.value === "meeting") { void refreshMeetings(); void refreshVirtualAudio(); } }}>
            <option value="mic">本机麦克风</option><option value="meeting">会议音频</option>
          </select></label>
          {inputSource === "mic" && <small>不用会议或直播：直接对麦克风说话，检测到停顿自动提交给角色；AI 播报时自动抑制回声。{micActive ? "麦克风已开启。" : ""}</small>}
          {inputSource === "meeting" && <>
            <label>会议进程<select value={meetingPid} onChange={(event) => setMeetingPid(event.target.value)}>
              <option value="">请选择会议进程</option>
              {meetingProcesses.map((process) => <option key={process.pid} value={process.pid}>{MEETING_NAMES[process.name.toLowerCase()] ?? process.name} · {process.title} · {process.pid}</option>)}
            </select></label>
            <button type="button" onClick={() => void refreshMeetings()}>刷新会议进程</button>
            <small>仅采集所选会议的音频，不采集屏幕。请告知参会者 AI 参与和转写；检测停顿后自动提交完整语句。</small>
            {selectedRoleScenario === "meetingAssistant" && <small>会议助手普通讨论只转写；被点名，或按 Ctrl+Alt+A 时才回答。</small>}
            {virtualAudio?.state === "missing" && <div className="preflight-card" role="alert">
              <span>检测到缺少虚拟声卡，是否安装并自动配置？</span>
              <button type="button" disabled={installingAudio || audioRetryBlocked} onClick={() => void installVirtualAudio()}>{installingAudio ? "正在安装…" : "是，自动安装"}</button>
            </div>}
            {installingAudio && <p role="status">{PREPARATION_PHASES[audioPreparationPhase]}… 请勿重复启动安装。</p>}
            {audioAttempted && !installingAudio && !virtualAudio?.installed && <small>最近安装步骤：{PREPARATION_PHASES[audioPreparationPhase]}。{audioRetryBlocked ? "请先重新检测，确认没有仍在运行的安装任务。" : "失败说明见页面提示；再次安装前会重新检查驱动状态。"}</small>}
            {!installingAudio && virtualAudio && !virtualAudio.installed && !["missing", "reboot_required"].includes(virtualAudio.state) && <div className="preflight-card" role="alert">{virtualAudio.detail}</div>}
            <button type="button" disabled={installingAudio} onClick={() => void refreshVirtualAudio()}>重新检测虚拟声卡</button>
            {virtualAudio?.rebootRequired && <div className="preflight-card" role="alert">虚拟声卡驱动已安装，需要重启 Windows 后继续。软件不会自动重启电脑。</div>}
            {virtualAudio?.installed && <small>虚拟声卡端点已就绪，将自动绑定音频线路；尚不代表会议对方已能听到声音。</small>}
          </>}
          {inputSource !== "meeting" && <><label>语音输出<select value={outputDeviceId} onChange={(event) => setOutputDeviceId(event.target.value)}>
            <option value="">系统默认输出</option>
            {audioOutputs.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
          </select></label>
          <button type="button" onClick={() => void refreshAudioOutputs()}>刷新音频设备</button>
            <small>本机麦克风使用 WebView 全双工播放和浏览器回声消除；所选输出同时作为原生兜底设备。</small>
          </>}

          <label>语音线路<select value={voiceRouteId} onChange={(event) => setVoiceRouteId(event.target.value)}>
            <option value="">请选择语音线路</option>
            {config.speech.voiceRoutes.filter((route) => route.configVersion > 0).map((route) => <option key={route.id} value={route.id}>{route.name} · {route.llmModelId ?? route.e2eModelId}</option>)}
          </select></label>
          <label><input type="checkbox" disabled={!canSearch} checked={allowWebSearch && canSearch} onChange={(event) => setAllowWebSearch(event.target.checked)} />允许本场联网搜索（可能产生费用）</label>
          {!canSearch && <small>联网问答：端到端线路需 DashScope Qwen3.8-Omni 系模型；级联线路需在模型供应商设置中选择支持的搜索协议。</small>}
          <label><input type="checkbox" disabled={busy || active} checked={allowBargeIn} onChange={(event) => setAllowBargeIn(event.target.checked)} />允许语音打断（说话即可停止 AI 播报）</label>
          <small>采集会议音频的会话会自动关闭打断；本机麦克风会话随时生效。</small>
        </fieldset>}
      </div>
    </header>
  );
}
