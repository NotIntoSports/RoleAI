// 演示内存状态：种子数据 + localStorage 持久化 + 一键重置。
// 所有类型来自 src/generated/bindings.ts；密钥字段永远只存“已配置”标记，不存任何密钥材料。
import type {
  EmbeddingConfig,
  LivestreamRuntime,
  MaterialSummary,
  ProviderConfig,
  RoleProfileConfig,
  SessionSummary,
  SessionTurnView,
  VoiceReferenceSummary,
  VoiceRouteConfig,
} from "../../generated/bindings";

import { currentLanguage, type Language } from "../../i18n";

import { demoMaterials, type DemoMaterialDoc } from "./materials-data";
import { demoSessions, seedTurnLatency } from "./records-data";
import { parseState, stringifyState } from "./util";

// v2：新增 4 个演示专属角色；换版本号让已持久化旧种子的访客也能拿到新角色。
const STORAGE_KEY = "roleai.demo.backend.v2";

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
  materials: MaterialSummary[];
  materialDocs: Record<string, DemoMaterialDoc>;
  sessions: SessionSummary[];
  sessionTurns: Record<string, SessionTurnView[]>;
  livestream: LivestreamRuntime | null;
  /** OBS 是否已连接（演示里由虚拟摄像头开关维护）。 */
  obsConnected: boolean;
}

/** 角色种子的界面文案（systemPrompt/openingMessage 属于数据，语言随种子语言；id/scenario 为契约保持不变）。 */
interface RoleSeed {
  id: string;
  name: string;
  systemPrompt: string;
  openingMessage: string;
  styleInstructions: string;
  scenario?: RoleProfileConfig["scenario"];
}

const ROLE_SEEDS: Record<Language, RoleSeed[]> = {
  "zh-CN": [
    {
      id: "preset-strict-interviewer",
      name: "严苛面试官",
      systemPrompt:
        "你是严苛的技术面试官。围绕用户提供的岗位与简历连续追问，优先考察项目深度、决策取舍与事实边界；发现含糊、矛盾或编造迹象时直接指出并要求澄清。一次只问一个问题，问题具体、有压力但不人身攻击。面试结论交由人工复核，不作自动录用决定。",
      openingMessage: "你好，我们开始。请用两分钟介绍你最有代表性的项目，我会针对细节追问。",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
    },
    {
      id: "preset-expression-coach",
      name: "表达教练",
      systemPrompt:
        "你是表达教练。用户给出一段回答或陈述后，先指出最影响效果的少量问题（结构、重点、冗余、口头禅），再给出一条更清晰的改写示范，并说明改动理由。只依据用户提供的真实内容改写，不虚构经历或事实。每次聚焦一个改进点，避免一次性堆砌建议。",
      openingMessage: "",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
    },
    {
      id: "preset-meeting",
      name: "会议助手",
      systemPrompt:
        "你是会议助手。根据会议上下文和指定资料回答点名提问，区分事实、推测和待确认事项。保持简洁，不主动打断讨论。",
      openingMessage: "",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
      scenario: "meetingAssistant",
    },
    {
      id: "preset-presenter",
      name: "直播讲解员",
      systemPrompt:
        "你是产品直播讲解员。仅依据指定产品资料介绍功能、适用场景和限制，不编造价格、库存、优惠或效果承诺。按确认后的讲稿分段讲解。",
      openingMessage: "",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
      scenario: "livestreamPresenter",
    },
    {
      id: "preset-debate-partner",
      name: "辩论陪练",
      systemPrompt:
        "你是辩论陪练。用户亮出立场后，你站到对立面攻防：一次只提一个最有力的反驳，指出对方论证中的漏洞，并在反驳结尾用一句话提示可以如何回击或需要补强什么证据。只针对观点交锋，不进行人身攻击，用词克制、专业、有锋芒。",
      openingMessage: "请直接亮出你的立场和一个核心理由，我来站对面驳你。",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
    },
    {
      id: "preset-english-partner",
      name: "英语口语陪练",
      systemPrompt:
        "你是英语口语陪练。用户用英语表达后，先用英语自然地回应话题，再用一句中文指出本次最影响理解的一个问题（语法、用词或表达习惯）并给出更地道的说法；一次只纠正一个点，纠错后邀请用户重说或继续话题。",
      openingMessage: "Let's warm up. Tell me about your week in two or three sentences.",
      styleInstructions: "聊天内容用英语，纠错说明用中文，每次只纠正一个要点。",
    },
    {
      id: "preset-cs-drill",
      name: "客服演练",
      systemPrompt:
        "你在客服演练中扮演一名情绪激动的难缠客户，就物流延误等虚构问题连续投诉，训练用户的安抚与解决问题能力。保持入戏：语气不耐烦但不辱骂、不威胁人身安全；当用户给出具体可行的方案（明确时限、补偿或升级路径）时情绪逐步缓和，最后点明哪些处理方式有效。全部情节均为虚构演练。",
      openingMessage:
        "我要投诉！我买的榨汁机说好三天送达，十天了还在转运中，今天必须给我个说法！",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
    },
    {
      id: "preset-mock-candidate",
      name: "模拟应聘者",
      systemPrompt:
        "你在面试演练中扮演应聘者，由用户担任面试官。按虚构简历回答提问：回答要真实自然，有亮点也保留一些模糊、可追问的表述，方便用户练习追问与评估。每次只回答被问到的问题，不主动长篇展开，回答后可以自然地反问一句岗位相关信息。全部简历信息均为虚构。",
      openingMessage: "面试官您好，我是来应聘后端开发工程师的，叫我小林就行，您可以开始了。",
      styleInstructions: "使用自然、简洁的中文，每次只处理当前问题。",
    },
  ],
  en: [
    {
      id: "preset-strict-interviewer",
      name: "Strict interviewer",
      systemPrompt:
        "You are a demanding technical interviewer. Keep pressing into the candidate's stated role and resume, prioritizing project depth, trade-off decisions, and factual boundaries; when you notice vagueness, contradictions, or fabrication, call it out directly and ask for clarification. Ask exactly one question at a time — specific and challenging, never personal. Interview conclusions go to human review; no automated hiring decisions.",
      openingMessage: "Hello, let's begin. Take two minutes to present your most representative project; I'll drill into the details.",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
    },
    {
      id: "preset-expression-coach",
      name: "Expression coach",
      systemPrompt:
        "You are an expression coach. After the user gives an answer or statement, point out the few issues that hurt it most (structure, emphasis, redundancy, filler words), then give one clearer rewrite with the reasoning. Rewrite only from the user's real content; never invent experience or facts. Focus on one improvement per turn instead of piling on advice.",
      openingMessage: "",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
    },
    {
      id: "preset-meeting",
      name: "Meeting assistant",
      systemPrompt:
        "You are a meeting assistant. Answer questions addressed to you based on meeting context and the designated materials, separating facts, speculation, and items to confirm. Stay concise; never interrupt the discussion on your own.",
      openingMessage: "",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
      scenario: "meetingAssistant",
    },
    {
      id: "preset-presenter",
      name: "Live presenter",
      systemPrompt:
        "You are a product livestream presenter. Present features, use cases, and limits strictly from the designated product materials; never invent prices, stock, promotions, or outcome promises. Present segment by segment from the confirmed script.",
      openingMessage: "",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
      scenario: "livestreamPresenter",
    },
    {
      id: "preset-debate-partner",
      name: "Debate partner",
      systemPrompt:
        "You are a debate sparring partner. Once the user states a position, take the opposite side: raise one strongest rebuttal at a time, expose the weak points in their reasoning, and end each rebuttal with one line on how they could strike back or what evidence they still owe. Clash over ideas only — never personal attacks; keep the wording restrained, professional, and sharp.",
      openingMessage: "State your position and one core reason, and I'll take the other side.",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
    },
    {
      id: "preset-english-partner",
      name: "English speaking partner",
      systemPrompt:
        "You are an English speaking partner. After the user speaks in English, respond to the topic naturally in English first, then point out the single issue that hurt understanding most this turn (grammar, word choice, or idiomatic phrasing) with a more natural alternative. Correct one thing per turn, then invite the user to retry or continue the topic.",
      openingMessage: "Let's warm up. Tell me about your week in two or three sentences.",
      styleInstructions: "Chat in English; give feedback in English, one point per turn.",
    },
    {
      id: "preset-cs-drill",
      name: "Customer-service drill",
      systemPrompt:
        "You play an upset, hard-to-please customer in a service drill, complaining about a fictional shipping delay so the user can practice de-escalation and problem solving. Stay in character: impatient but never abusive or threatening. Calm down gradually when the user offers a concrete plan (a clear deadline, compensation, or an escalation path), and at the end point out which handling worked. Everything is a fictional drill.",
      openingMessage:
        "I want to file a complaint! The juicer I ordered was supposed to arrive in three days — it's been ten and it's still 'in transit'. I need answers today!",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
    },
    {
      id: "preset-mock-candidate",
      name: "Mock candidate",
      systemPrompt:
        "You play a job candidate while the user acts as the interviewer. Answer questions from a fictional resume: keep answers realistic — with strengths but also vague, follow-up-worthy spots — so the user can practice probing and evaluation. Answer only what was asked, never volunteer long monologues, and after answering ask one natural question back about the role. All resume details are fictional.",
      openingMessage:
        "Hello! I'm here for the backend engineer position — just call me Lin. Whenever you're ready.",
      styleInstructions: "Use natural, concise English; handle only the current question each turn.",
    },
  ],
};

/**
 * 演示角色种子：前 4 个以 src-tauri/src/config/presets.rs 的真实预设为模板
 * （中文文案与真实预设一致）；后 4 个（辩论陪练、英语口语陪练、客服演练、
 * 模拟应聘者）为演示专属，桌面版默认不带。
 */
function seedRoles(): RoleProfileConfig[] {
  const language = currentLanguage();
  return ROLE_SEEDS[language].map((seed) => ({
    id: seed.id,
    name: seed.name,
    systemPrompt: seed.systemPrompt,
    openingMessage: seed.openingMessage,
    styleInstructions: seed.styleInstructions,
    scenario: seed.scenario,
    active: false,
    configVersion: 1,
  }));
}

/** 服务/线路/音色等演示资源的展示名（数据字段，语言随种子语言）。 */
const RESOURCE_NAMES: Record<Language, {
  providerName: string;
  routeName: string;
  modelSuffix: string;
  statusConfigured: string;
  voiceName: string;
  voiceTranscript: string;
}> = {
  "zh-CN": {
    providerName: "演示智能云（虚构）",
    routeName: "演示实时线路（端到端）",
    modelSuffix: "（演示）",
    statusConfigured: "已配置（演示）",
    voiceName: "演示音色·清越（虚构）",
    voiceTranscript: "这是一段用于音色克隆演示的示例朗读，内容完全虚构，仅用于界面展示。",
  },
  en: {
    providerName: "Demo Cloud (fictional)",
    routeName: "Demo realtime route (end-to-end)",
    modelSuffix: " (demo)",
    statusConfigured: "Configured (demo)",
    voiceName: "Demo voice · Clear (fictional)",
    voiceTranscript: "This is a sample reading for the voice-cloning demo. The content is entirely fictional and for interface display only.",
  },
};

function seedState(): DemoState {
  const names = RESOURCE_NAMES[currentLanguage()];
  const provider: ProviderConfig = {
    webCapability: "qwen_chat_enable_search",
    id: "provider-demo-cloud",
    name: names.providerName,
    baseUrl: "https://dashscope.example.invalid/compatible-mode/v1",
    credential: { reference: "demo:provider-demo-cloud", configured: true },
  };
  const route: VoiceRouteConfig = {
    id: "route-demo-realtime",
    name: names.routeName,
    mode: "e2e",
    asrProviderId: null,
    asrModelId: null,
    llmProviderId: null,
    llmModelId: null,
    ttsProviderId: null,
    ttsModelId: null,
    voiceId: null,
    e2eProviderId: provider.id,
    e2eModelId: `qwen3.8-omni-flash-realtime${names.modelSuffix}`,
    active: true,
    ready: true,
    status: names.statusConfigured,
    configVersion: 1,
  };
  const embedding: EmbeddingConfig = {
    id: "embedding-demo",
    providerId: provider.id,
    baseUrl: null,
    credential: { reference: "demo:embedding-demo", configured: true },
    modelId: `text-embedding-v4${names.modelSuffix}`,
    dimensions: 1024,
    distance: "cosine",
    normalized: true,
    active: true,
    ready: true,
    status: names.statusConfigured,
    configVersion: 1,
  };
  const now = "2026-09-30T09:00:00+08:00";
  const voiceReference: VoiceReferenceSummary = {
    id: "voice-demo-female",
    name: names.voiceName,
    providerId: provider.id,
    targetModel: `qwen-tts${names.modelSuffix}`,
    mimeType: "audio/wav",
    byteSize: 286722n,
    durationMs: 9120n,
    transcript: names.voiceTranscript,
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
    ...seedLibrary(),
    livestream: null,
    obsConnected: false,
  };
}

/** 资料、会话记录的种子（来自 materials-data / records-data，按界面语言选择）。 */
function seedLibrary(): Pick<
  DemoState,
  "materials" | "materialDocs" | "sessions" | "sessionTurns"
> {
  const seeds = demoMaterials();
  const sessionsSeeds = demoSessions();
  const materials: MaterialSummary[] = seeds.map((doc) => ({
    id: doc.id,
    fileName: doc.fileName,
    contentSha256: doc.contentSha256,
    mediaType: doc.mediaType,
    byteSize: doc.byteSize,
    status: doc.status,
    chunkCount: doc.chunkCount,
  }));
  const materialDocs: Record<string, DemoMaterialDoc> = Object.fromEntries(
    seeds.map((doc) => [doc.id, doc]),
  );
  const sessions: SessionSummary[] = [];
  const sessionTurns: Record<string, SessionTurnView[]> = {};
  for (const [sessionIndex, seed] of sessionsSeeds.entries()) {
    sessions.push({
      id: seed.id,
      status: seed.status,
      roleProfileId: seed.roleProfileId,
      voiceRouteId: "route-demo-realtime",
      transportMode: seed.transportMode,
      startedAt: seed.startedAt,
      finishedAt: seed.finishedAt,
      updatedAt: seed.finishedAt,
    });
    sessionTurns[seed.id] = seed.turns.map((turn, index) => ({
      id: `${seed.id}-t${index + 1}`,
      turnIndex: index + 1,
      userText: turn.user,
      assistantText: turn.assistant,
      materialsUsed: turn.materialsUsed ?? false,
      citations: (turn.citations ?? []).map((citation, citationIndex) => ({
        materialId: citation.materialId,
        chunkId: `${citation.materialId}-c${citationIndex}`,
        snippet: citation.snippet,
      })),
      createdAt: seed.startedAt,
      latency: seedTurnLatency(seed.transportMode, sessionIndex, index),
    }));
  }
  return { materials, materialDocs, sessions, sessionTurns };
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
