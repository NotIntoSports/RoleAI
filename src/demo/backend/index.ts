// 演示版模拟后端（内存实现）：拦截全部 Tauri 命令，返回虚构数据。
// 约束：
// - 类型全部来自 src/generated/bindings.ts（DTO 变化时这里会在编译期报错）；
// - 不发任何网络请求，密钥字段永远只存“已配置”标记；
// - 领域拆分：state（状态/持久化）、config（公共配置/诊断）、roles（角色）、
//   providers（供应商/Embedding）、voice（语音线路/音色/OBS 密钥）。
import type {
  AudioOutputDevice,
  CommandResult,
  LegacySessionImport,
  MeetingProcess,
  ObsRuntimeStatus,
  RuntimeStatus,
  VirtualAudioPreparation,
} from "../../generated/bindings";

import { handleConfigCommand } from "./config";
import { handleLiveSessionCommand, liveRuntimeStatus } from "./live-commands";
import { handleLivestreamCommand } from "./livestream";
import { handleMaterialCommand } from "./materials";
import { handlePracticeCommand, resetPracticeState } from "./practice";
import { handleProviderCommand } from "./providers";
import { handleRecordCommand } from "./records";
import { handleRoleCommand } from "./roles";
import { getState, resetDemoState } from "./state";
import { handleVoiceCommand } from "./voice";
import { err, latency, ok } from "./util";

import { demoT } from "./demo-text";

// 与 mockIPC 回调的入参类型对齐（官方 InvokeArgs 的结构副本；
// 不直接 import，前端契约测试只允许 src/api/commands.ts 引用 core 模块）。
type DemoPayload = Record<string, unknown> | number[] | ArrayBuffer | Uint8Array;

function idleRuntime(): RuntimeStatus {
  return {
    phase: "idle",
    mode: "ai_active",
    seq: 0,
    unusedMaterials: false,
    lastErrorCode: null,
    revision: 0,
    realtimeStatus: "idle",
  };
}

const OBS_IDLE: ObsRuntimeStatus = {
  connected: false,
  sceneReady: false,
  browserSourceReady: false,
  virtualCameraActive: false,
  errorCode: null,
};

const DEMO_MEETINGS: MeetingProcess[] = [
  { pid: 41001, name: "ms-teams.exe", title: demoT().misc.meetingTitle },
];

const DEMO_AUDIO_OUTPUTS: AudioOutputDevice[] = [
  { id: "demo-output-default", name: demoT().misc.speakerName },
];

function handleMeetingAndAudio(cmd: string): unknown {
  switch (cmd) {
    case "meeting_process_list":
      return latency(80, 220).then(() => ok(DEMO_MEETINGS));
    case "audio_output_list":
      return latency(30, 100).then(() => ok(DEMO_AUDIO_OUTPUTS));
    case "virtual_audio_status":
      return ok({
        state: "ready",
        installed: true,
        rebootRequired: false,
        detail: demoT().misc.virtualAudioReady,
        renderEndpointId: "demo-virtual-render",
        captureEndpointId: "demo-virtual-capture",
      } satisfies VirtualAudioPreparation);
    case "virtual_audio_install":
      return latency(300, 700).then(() =>
        ok({
          state: "ready",
          installed: true,
          rebootRequired: false,
          detail: demoT().misc.virtualAudioNoInstall,
          renderEndpointId: "demo-virtual-render",
          captureEndpointId: "demo-virtual-capture",
        } satisfies VirtualAudioPreparation),
      );
    default:
      return undefined;
  }
}

/** 领域分发：config / roles / providers / voice / 资料 / 记录 / 会议与音频 / 模拟面试训练 / 其余兜底。 */
function dispatch(cmd: string, payload: Record<string, unknown>): unknown {
  return (
    handleConfigCommand(cmd, payload) ??
    handleRoleCommand(cmd, payload) ??
    handleProviderCommand(cmd, payload) ??
    handleVoiceCommand(cmd, payload) ??
    handleMaterialCommand(cmd, payload) ??
    handleRecordCommand(cmd, payload) ??
    handleLivestreamCommand(cmd, payload) ??
    handlePracticeCommand(cmd, payload) ??
    handleMeetingAndAudio(cmd) ??
    err("DEMO_NOT_IMPLEMENTED", demoT().misc.notImplemented(cmd))
  );
}

/** mockIPC 回调入口。所有返回值统一为 CommandResult<T>（或其 Promise）。 */
export function handleDemoInvoke(cmd: string, payload?: DemoPayload): unknown {
  const args: Record<string, unknown> =
    payload && typeof payload === "object" && !ArrayBuffer.isView(payload) && !Array.isArray(payload)
      ? (payload as Record<string, unknown>)
      : {};
  switch (cmd) {
    case "runtime_get_status": {
      const live = liveRuntimeStatus();
      if (live) return ok(live);
      return latency(10, 50).then(() => ok(idleRuntime()));
    }
    case "legacy_import_source":
      return latency(200, 500).then(() => ok({ sessions: 0, turns: 0 } satisfies LegacySessionImport));
    default: {
      // 实时会话命令优先于记录/资料等静态领域。
      const liveResult = handleLiveSessionCommand(cmd, args);
      if (liveResult !== undefined) return liveResult;
      return dispatch(cmd, args);
    }
  }
}

/** 供 e2e 与手动重置使用：清空演示数据并刷新页面。 */
export function installDemoResetHook(): void {
  (window as { __roleaiDemoReset?: () => void }).__roleaiDemoReset = () => {
    resetDemoState();
    resetPracticeState();
    window.location.reload();
  };
  // 读取一次状态，保证首次渲染前种子已就绪。
  getState();
}
