import { ChevronDown, Wrench } from "lucide-react";

import type {
  AudioOutputDevice,
  MeetingProcess,
  PublicConfig,
  RoleScenario,
  VirtualAudioPreparation,
} from "../../generated/bindings";
import { t, useT } from "../../i18n";

const PHASE_KEYS = ["idle", "preparing", "listening", "thinking", "speaking", "stopping", "recovering", "blocked", "completed", "failed"] as const;
type KnownPhase = (typeof PHASE_KEYS)[number];

const MODE_KEYS = ["ai_active", "operator_speaking", "paused", "muted"] as const;
type KnownMode = (typeof MODE_KEYS)[number];

const MEETING_NAME_KEYS: Record<string, "session.meetingNames.teams" | "session.meetingNames.ms-teams" | "session.meetingNames.wemeet" | "session.meetingNames.feishu" | "session.meetingNames.lark" | "session.meetingNames.dingtalk" | "session.meetingNames.zoom"> = {
  "teams.exe": "session.meetingNames.teams", "ms-teams.exe": "session.meetingNames.ms-teams",
  "wemeetapp.exe": "session.meetingNames.wemeet", "feishu.exe": "session.meetingNames.feishu", "lark.exe": "session.meetingNames.lark",
  "dingtalk.exe": "session.meetingNames.dingtalk", "zoom.exe": "session.meetingNames.zoom",
};

const PREPARATION_KEYS = ["checking", "downloading", "verifying", "authorizing", "installing", "rechecking"] as const;
type KnownPreparation = (typeof PREPARATION_KEYS)[number];

function phaseLabel(phase: string): string {
  return (PHASE_KEYS as readonly string[]).includes(phase) ? t(`session.phases.${phase as KnownPhase}`) : phase;
}

function modeLabel(mode: string): string {
  return (MODE_KEYS as readonly string[]).includes(mode) ? t(`session.modes.${mode as KnownMode}`) : mode;
}

function meetingName(processName: string): string {
  const key = MEETING_NAME_KEYS[processName.toLowerCase()];
  return key ? t(key) : processName;
}

function preparationLabel(phase: string): string {
  return (PREPARATION_KEYS as readonly string[]).includes(phase) ? t(`session.preparation.${phase as KnownPreparation}`) : phase;
}

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
  /** 后端返回 PLATFORM_UNSUPPORTED 后置位：会议采集与原生输出设备在当前平台不可用。 */
  meetingUnsupported?: boolean;
  outputsUnsupported?: boolean;
  canSearch: boolean;
  allowWebSearch: boolean;
  allowBargeIn: boolean;
  setAllowBargeIn: (allow: boolean) => void;
  onTriggerAssistant: () => void;
  /** 最后一轮有用户文本、没有回答：存在可追答的发言，按钮加 data-pending 高亮。 */
  assistantPending: boolean;
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
  meetingUnsupported = false,
  outputsUnsupported = false,
  canSearch,
  allowWebSearch,
  allowBargeIn,
  setAllowBargeIn,
  onTriggerAssistant,
  assistantPending,
}: SessionToolbarProps) {
  useT();
  const meetingGating = active && inputSource === "meeting" && selectedRoleScenario === "meetingAssistant";
  return (
    <header className="session-toolbar">
      <div className="session-toolbar-meta">
        <h2 id="workspace-session-heading">{t("session.toolbar.heading")}</h2>
        <span className="status-badge" data-active={active}>
          {phaseLabel(phase)}
        </span>
        {realtimeStatus === "reconnecting" && <span className="status-badge" data-active={active}>{t("session.toolbar.reconnecting")}</span>}
        {realtimeStatus === "failed" && <span className="status-badge" data-active={false}>{t("session.toolbar.connectFailed")}</span>}
        <span className="session-mode">{modeLabel(mode)}</span>
        {/* 会话配置 fieldset 在会话进行中整体禁用，按钮必须放外面才可点。 */}
        {meetingGating && (
          <button type="button" className="button-primary" data-pending={assistantPending} disabled={busy} onClick={onTriggerAssistant}>{t("session.toolbar.triggerAssistant")}</button>
        )}
      </div>
      {/* 门控常驻提示：放在「会话配置」折叠区之外，会话进行中始终可见。 */}
      {meetingGating && (
        <p className="session-gating-hint">{t("session.toolbar.gatingHint")}</p>
      )}
      <div className="session-config-heading">
        <span className="session-config-summary">{inputSource === "meeting" ? t("session.toolbar.meetingAudio") : t("session.toolbar.localMic")} · {config?.speech.voiceRoutes.find((route) => route.id === voiceRouteId)?.name ?? t("session.toolbar.noRouteSelected")}</span>
        <button type="button" className="button-ghost" aria-expanded={configurationOpen} aria-controls="session-configuration" onClick={() => setConfigurationOpen(!configurationOpen)}><Wrench size={15} aria-hidden="true" />{t("session.toolbar.configToggle")}<ChevronDown size={14} aria-hidden="true" /></button>
      </div>
      <div id="session-configuration" className="session-configuration" hidden={!configurationOpen}>
        {!config && <p className="muted">{t("session.toolbar.noConfigLoaded")}</p>}
        {config && <fieldset disabled={busy || active} className="session-selection">
          <legend>{t("session.toolbar.fieldsetLegend")}</legend>
          <label>{t("session.toolbar.inputSource")}<select value={inputSource} onChange={(event) => { setInputSource(event.target.value); if (event.target.value === "meeting") { void refreshMeetings(); void refreshVirtualAudio(); } }}>
            <option value="mic">{t("session.toolbar.localMic")}</option><option value="meeting" disabled={meetingUnsupported}>{meetingUnsupported ? t("session.toolbar.meetingAudioUnsupported") : t("session.toolbar.meetingAudio")}</option>
          </select></label>
          {inputSource === "mic" && <small>{t("session.toolbar.micHint")}{micActive ? t("session.toolbar.micActiveNote") : ""}</small>}
          {inputSource === "meeting" && <>
            <label>{t("session.toolbar.meetingProcess")}<select value={meetingPid} onChange={(event) => setMeetingPid(event.target.value)}>
              <option value="">{t("session.toolbar.pickMeeting")}</option>
              {meetingProcesses.map((process) => <option key={process.pid} value={process.pid}>{meetingName(process.name)} · {process.title} · {process.pid}</option>)}
            </select></label>
            <button type="button" onClick={() => void refreshMeetings()}>{t("session.toolbar.refreshMeetings")}</button>
            <small>{t("session.toolbar.meetingPrivacyNote")}</small>
            {selectedRoleScenario === "meetingAssistant" && <small>{t("session.toolbar.meetingAssistantNote")}</small>}
            {virtualAudio?.state === "missing" && <div className="preflight-card" role="alert">
              <span>{t("session.toolbar.installAsk")}</span>
              <button type="button" disabled={installingAudio || audioRetryBlocked} onClick={() => void installVirtualAudio()}>{installingAudio ? t("session.toolbar.installingNow") : t("session.toolbar.installYes")}</button>
            </div>}
            {installingAudio && <p role="status">{t("session.toolbar.preparingNotice", { phase: preparationLabel(audioPreparationPhase) })}</p>}
            {audioAttempted && !installingAudio && !virtualAudio?.installed && <small>{t("session.toolbar.lastInstallStep", { phase: preparationLabel(audioPreparationPhase) })} {audioRetryBlocked ? t("session.toolbar.retryBlockedNote") : t("session.toolbar.retryAllowedNote")}</small>}
            {!installingAudio && virtualAudio && !virtualAudio.installed && !["missing", "reboot_required"].includes(virtualAudio.state) && <div className="preflight-card" role="alert">{virtualAudio.detail}</div>}
            <button type="button" disabled={installingAudio} onClick={() => void refreshVirtualAudio()}>{t("session.toolbar.recheckAudio")}</button>
            {virtualAudio?.rebootRequired && <div className="preflight-card" role="alert">{t("session.toolbar.rebootRequired")}</div>}
            {virtualAudio?.installed && <small>{t("session.toolbar.virtualAudioReady")}</small>}
          </>}
          {inputSource !== "meeting" && !outputsUnsupported && <><label>{t("session.toolbar.voiceOutput")}<select value={outputDeviceId} onChange={(event) => setOutputDeviceId(event.target.value)}>
            <option value="">{t("session.toolbar.systemDefault")}</option>
            {audioOutputs.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
          </select></label>
          <button type="button" onClick={() => void refreshAudioOutputs()}>{t("session.toolbar.refreshAudioDevices")}</button>
            <small>{t("session.toolbar.micOutputNote")}</small>
          </>}

          <label>{t("session.toolbar.voiceRoute")}<select value={voiceRouteId} onChange={(event) => setVoiceRouteId(event.target.value)}>
            <option value="">{t("session.toolbar.pickRoute")}</option>
            {config.speech.voiceRoutes.filter((route) => route.configVersion > 0).map((route) => <option key={route.id} value={route.id}>{route.name} · {route.llmModelId ?? route.e2eModelId}</option>)}
          </select></label>
          <label><input type="checkbox" disabled={!canSearch} checked={allowWebSearch && canSearch} onChange={(event) => setAllowWebSearch(event.target.checked)} />{t("session.toolbar.allowWebSearch")}</label>
          {!canSearch && <small>{t("session.toolbar.webSearchNote")}</small>}
          <label><input type="checkbox" disabled={busy || active} checked={allowBargeIn} onChange={(event) => setAllowBargeIn(event.target.checked)} />{t("session.toolbar.allowBargeIn")}</label>
          <small>{t("session.toolbar.bargeInNote")}</small>
        </fieldset>}
      </div>
    </header>
  );
}
