// 演示版模拟后端（内存实现）：拦截全部 Tauri 命令，返回虚构数据。
// 约束：
// - 类型全部来自 src/generated/bindings.ts（DTO 变化时这里会在编译期报错）；
// - 不发任何网络请求，密钥字段永远为空；
// - B01 最小集：让六个页面都能打开；领域数据在后续卡片充实。
import type {
  CommandResult,
  FoundationStatus,
  LegacyMigrationStatus,
  LivestreamRuntime,
  ObsRuntimeStatus,
  PublicConfig,
  RuntimeStatus,
  SecretStatus,
} from "../../generated/bindings";

// 与 mockIPC 回调的入参类型对齐（官方 InvokeArgs 的结构副本；
// 不直接 import，前端契约测试只允许 src/api/commands.ts 引用 core 模块）。
type DemoPayload = Record<string, unknown> | number[] | ArrayBuffer | Uint8Array;

const ok = <T,>(data: T): CommandResult<T> => ({ ok: true, data });
const err = (code: string, message: string): CommandResult<never> => ({
  ok: false,
  error: { code, message, requestId: "demo", retryable: false },
});

const ready: FoundationStatus = { ready: true };

function demoConfig(): PublicConfig {
  return {
    configVersion: 1,
    application: { locale: null },
    models: { providers: [], activeProviderId: null },
    speech: { voiceRoutes: [], activeVoiceRouteId: null },
    knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
    storage: { exportDirectory: null },
    roleProfiles: [],
    activeRoleProfileId: null,
    diagnostics: { logRetentionDays: 14 },
  };
}

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

function emptyLivestream(): LivestreamRuntime {
  return {
    script: {
      id: "demo-script",
      title: "演示讲稿",
      segments: [],
      loopEnabled: false,
      confirmed: false,
      currentIndex: null,
      state: "draft",
    },
    stage: {
      productTitle: "演示讲稿",
      currentSubtitle: "",
      nextHint: "",
      state: "draft",
      mediaPath: null,
      mediaKind: null,
      outputState: "idle",
      outputErrorCode: null,
    },
  };
}

const OBS_IDLE: ObsRuntimeStatus = {
  connected: false,
  sceneReady: false,
  browserSourceReady: false,
  virtualCameraActive: false,
  errorCode: null,
};

const OBS_SECRET: SecretStatus = { reference: "demo:obs", configured: false };

const MIGRATION: LegacyMigrationStatus = { applied: true, reenterSecrets: false, omitted: [] };

/** demo 命令分发器。未知命令返回统一错误（不抛异常，界面只显示提示）。 */
export function handleDemoInvoke(cmd: string, _payload?: DemoPayload): unknown {
  switch (cmd) {
    case "config_get_startup_state":
      return ok({ kind: "ready" });
    case "config_get_public":
      return ok(demoConfig());
    case "config_restore_last_good":
    case "config_restore_defaults":
      return ok({ kind: "ready" });
    case "legacy_migration_status":
      return ok(MIGRATION);
    case "foundation_get_status":
    case "open_app_directory":
    case "open_web_source":
    case "session_audio_ready":
    case "session_delete":
      return ok(ready);
    case "runtime_get_status":
      return ok(idleRuntime());
    case "session_list":
      return ok([]);
    case "material_list":
      return ok([]);
    case "voice_reference_list":
      return ok([]);
    case "livestream_get":
      return ok(emptyLivestream());
    case "obs_runtime_status":
      return ok(OBS_IDLE);
    case "obs_password_status":
      return ok(OBS_SECRET);
    case "diagnostics_latency_summary":
      return ok({ sessionsScanned: 0, routes: [] });
    case "diagnostics_export":
      return ok({ exported: false });
    default:
      // 后续卡片（B03/B04/B05）逐域补齐；这里兜底保证演示不崩。
      return err("DEMO_NOT_IMPLEMENTED", `在线演示尚未模拟该能力（${cmd}）`);
  }
}
