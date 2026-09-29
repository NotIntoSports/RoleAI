// 演示内存状态：种子数据 + localStorage 持久化 + 一键重置。
// 所有类型来自 src/generated/bindings.ts；密钥字段永远只存“已配置”标记，不存任何密钥材料。
import type {
  EmbeddingConfig,
  ProviderConfig,
  RoleProfileConfig,
  VoiceReferenceSummary,
  VoiceRouteConfig,
} from "../../generated/bindings";

import { parseState, stringifyState } from "./util";

const STORAGE_KEY = "roleai.demo.backend.v1";

export interface DemoState {
  configVersion: number;
  providers: ProviderConfig[];
  activeProviderId: string | null;
  /** providerId -> 密钥是否已配置（演示里恒为 true 的脱敏标记）。 */
  providerSecrets: Record<string, boolean>;
  voiceRoutes: VoiceRouteConfig[];
  activeVoiceRouteId: string | null;
  embeddingConfigs: EmbeddingConfig[];
  activeEmbeddingConfigId: string | null;
  roleProfiles: RoleProfileConfig[];
  activeRoleProfileId: string | null;
  voiceReferences: VoiceReferenceSummary[];
  obsPasswordConfigured: boolean;
}

const STYLE = "使用自然、简洁的中文，每次只处理当前问题。";

/** 以 src-tauri/src/config/presets.rs 的真实预设为模板的 4 个演示角色（文案与真实预设一致）。 */
function seedRoles(): RoleProfileConfig[] {
  return [
    {
      id: "preset-strict-interviewer",
      name: "严苛面试官",
      systemPrompt:
        "你是严苛的技术面试官。围绕用户提供的岗位与简历连续追问，优先考察项目深度、决策取舍与事实边界；发现含糊、矛盾或编造迹象时直接指出并要求澄清。一次只问一个问题，问题具体、有压力但不人身攻击。面试结论交由人工复核，不作自动录用决定。",
      openingMessage: "你好，我们开始。请用两分钟介绍你最有代表性的项目，我会针对细节追问。",
      styleInstructions: STYLE,
      active: false,
      configVersion: 1,
    },
    {
      id: "preset-expression-coach",
      name: "表达教练",
      systemPrompt:
        "你是表达教练。用户给出一段回答或陈述后，先指出最影响效果的少量问题（结构、重点、冗余、口头禅），再给出一条更清晰的改写示范，并说明改动理由。只依据用户提供的真实内容改写，不虚构经历或事实。每次聚焦一个改进点，避免一次性堆砌建议。",
      openingMessage: "",
      styleInstructions: STYLE,
      active: false,
      configVersion: 1,
    },
    {
      id: "preset-meeting",
      name: "会议助手",
      systemPrompt:
        "你是会议助手。根据会议上下文和指定资料回答点名提问，区分事实、推测和待确认事项。保持简洁，不主动打断讨论。",
      openingMessage: "",
      styleInstructions: STYLE,
      scenario: "meetingAssistant",
      active: false,
      configVersion: 1,
    },
    {
      id: "preset-presenter",
      name: "直播讲解员",
      systemPrompt:
        "你是产品直播讲解员。仅依据指定产品资料介绍功能、适用场景和限制，不编造价格、库存、优惠或效果承诺。按确认后的讲稿分段讲解。",
      openingMessage: "",
      styleInstructions: STYLE,
      scenario: "livestreamPresenter",
      active: false,
      configVersion: 1,
    },
  ];
}

function seedState(): DemoState {
  const provider: ProviderConfig = {
    webCapability: "qwen_chat_enable_search",
    id: "provider-demo-cloud",
    name: "演示智能云（虚构）",
    baseUrl: "https://dashscope.example.invalid/compatible-mode/v1",
    credential: { reference: "demo:provider-demo-cloud", configured: true },
  };
  const route: VoiceRouteConfig = {
    id: "route-demo-realtime",
    name: "演示实时线路（端到端）",
    mode: "e2e",
    asrProviderId: null,
    asrModelId: null,
    llmProviderId: null,
    llmModelId: null,
    ttsProviderId: null,
    ttsModelId: null,
    voiceId: null,
    e2eProviderId: provider.id,
    e2eModelId: "qwen3.8-omni-flash-realtime（演示）",
    active: true,
    ready: true,
    status: "已配置（演示）",
    configVersion: 1,
  };
  const embedding: EmbeddingConfig = {
    id: "embedding-demo",
    providerId: provider.id,
    baseUrl: null,
    credential: { reference: "demo:embedding-demo", configured: true },
    modelId: "text-embedding-v4（演示）",
    dimensions: 1024,
    distance: "cosine",
    normalized: true,
    active: true,
    ready: true,
    status: "已配置（演示）",
    configVersion: 1,
  };
  const now = "2026-09-30T09:00:00+08:00";
  const voiceReference: VoiceReferenceSummary = {
    id: "voice-demo-female",
    name: "演示音色·清越（虚构）",
    providerId: provider.id,
    targetModel: "qwen-tts（演示）",
    mimeType: "audio/wav",
    byteSize: 286722n,
    durationMs: 9120n,
    transcript: "这是一段用于音色克隆演示的示例朗读，内容完全虚构，仅用于界面展示。",
    remoteFileId: "demo-remote-file-1",
    voiceId: "demo-voice-qingyue",
    cloneStatus: "ready",
    cloneError: null,
    createdAt: now,
    updatedAt: now,
  };
  return {
    configVersion: 1,
    providers: [provider],
    activeProviderId: provider.id,
    providerSecrets: { [provider.id]: true },
    voiceRoutes: [route],
    activeVoiceRouteId: route.id,
    embeddingConfigs: [embedding],
    activeEmbeddingConfigId: embedding.id,
    roleProfiles: seedRoles(),
    activeRoleProfileId: "preset-strict-interviewer",
    voiceReferences: [voiceReference],
    obsPasswordConfigured: false,
  };
}

let state: DemoState | null = null;

function readStorage(): DemoState | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw ? parseState<DemoState>(raw) : null;
  } catch {
    return null;
  }
}

function writeStorage(next: DemoState): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, stringifyState(next));
  } catch {
    // 存储不可用（隐私模式等）时退化为纯内存状态。
  }
}

export function getState(): DemoState {
  if (!state) {
    state = readStorage() ?? seedState();
  }
  return state;
}

export function updateState(mutate: (draft: DemoState) => void): DemoState {
  const current = getState();
  mutate(current);
  current.configVersion += 1;
  writeStorage(current);
  return current;
}

/** 一键重置：丢弃本地持久化的演示数据并恢复种子。 */
export function resetDemoState(): void {
  state = seedState();
  writeStorage(state);
}

/** 供测试与开发核对：当前是否处于种子状态。 */
export function isSeedState(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === null;
  } catch {
    return true;
  }
}
