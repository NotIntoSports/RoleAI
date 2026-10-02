// 演示后端文案表：按界面语言（src/i18n 的 currentLanguage）在中文/英文之间取值。
// 约束：
// - 只承载演示后端动态返回给界面看的文案与数据种子选择；界面静态文案仍走 src/locales；
// - 命令错误码与后端契约值（如面试官风格/难度的 value）保持与桌面版一致，不在这里翻译；
// - 种子数据在播种时取当时语言；动态回复按命令调用时语言取值。
import { currentLanguage, type Language } from "../../i18n";

export function demoLang(): Language {
  return currentLanguage();
}

export interface DemoText {
  roles: {
    nameRequired: string;
    copySourceMissing: string;
    copySuffix: string;
    notFound: string;
  };
  providers: {
    endpointRequired: string;
    notFound: string;
    inUse: string;
    embeddingNotFound: string;
    statusConfigured: string;
    modelSuffix: string;
  };
  voice: {
    routeNameRequired: string;
    routeNotFound: string;
    routeNotReady: string;
    voiceNotFound: string;
    statusConfigured: string;
  };
  materials: {
    pathRequired: string;
    importSectionTitle: string;
    importSectionText: (fileName: string) => string;
  };
  records: {
    notFound: string;
    markdown: {
      sessionHeading: (id: string) => string;
      role: string;
      status: string;
      started: string;
      finished: string;
      transport: string;
      disclaimer: string;
      turnHeading: (n: number) => string;
      user: string;
      assistant: string;
      citations: string;
    };
    downloaded: (fileName: string) => string;
  };
  live: {
    noActiveSession: string;
    retryFallback: string;
    tailReplies: string[];
    reportFallbackRole: string;
    reportSummary: (turns: number, role: string) => string;
    reportStrengths: string[];
    reportFollowUps: string[];
    reportLimitations: string[];
  };
  misc: {
    demoScriptTitle: string;
    meetingTitle: string;
    speakerName: string;
    virtualAudioReady: string;
    virtualAudioNoInstall: string;
    notImplemented: (cmd: string) => string;
  };
  livestream: {
    inputInvalid: string;
    noMaterial: string;
    notConfirmed: string;
    unknownAction: (action: string) => string;
    questionEmpty: string;
    questionSegmentTitle: (question: string) => string;
    questionSegmentText: (question: string) => string;
  };
}

const ZH: DemoText = {
  roles: {
    nameRequired: "角色名称不能为空。",
    copySourceMissing: "找不到要复制的角色。",
    copySuffix: " 副本",
    notFound: "找不到该角色。",
  },
  providers: {
    endpointRequired: "接入地址不能为空。",
    notFound: "找不到该供应商。",
    inUse: "供应商仍被语音线路或 Embedding 配置引用。",
    embeddingNotFound: "找不到该 Embedding 配置。",
    statusConfigured: "已配置（演示）",
    modelSuffix: "（演示）",
  },
  voice: {
    routeNameRequired: "线路名称不能为空。",
    routeNotFound: "找不到该语音线路。",
    routeNotReady: "线路尚未通过测试，无法启用。",
    voiceNotFound: "找不到该音色。",
    statusConfigured: "已配置（演示）",
  },
  materials: {
    pathRequired: "请填写资料文件路径。",
    importSectionTitle: "导入内容",
    importSectionText: (fileName) =>
      `演示资料「${fileName}」。在线演示不会读取本地文件内容，这里只生成占位正文用于界面展示；桌面版会真实切片、嵌入并支持全文检索。`,
  },
  records: {
    notFound: "找不到该会话记录。",
    markdown: {
      sessionHeading: (id) => `会话记录 ${id}`,
      role: "角色",
      status: "状态",
      started: "开始",
      finished: "结束",
      transport: "传输",
      disclaimer: "在线演示数据，全部内容均为虚构。",
      turnHeading: (n) => `轮 ${n}`,
      user: "用户",
      assistant: "助手",
      citations: "引用片段：",
    },
    downloaded: (fileName) => `已通过浏览器下载：${fileName}（在线演示不写入本地磁盘）`,
  },
  live: {
    noActiveSession: "当前没有进行中的会话。",
    retryFallback: "（重答）好的，我们再讲一遍。",
    tailReplies: [
      "演示脚本到这里就播完了。桌面的真实版本会持续进行语音对话；在线演示里你可以输入文字继续体验，或结束会话回看记录。",
      "这段是演示的固定结尾：想再看一遍完整脚本，可以结束会话后重新开始；想体验真实语音，请下载桌面版。",
    ],
    reportFallbackRole: "演示角色",
    reportSummary: (turns, role) =>
      `本场景共进行 ${turns} 轮对话（角色：${role}）。在线演示评分为固定脚本演示，不代表模型真实评价；桌面版会基于真实对话生成评分报告。`,
    reportStrengths: [
      "核心概念边界清楚，能落到组件级方案",
      "面对追问能给出取舍理由，而不是只报结论",
    ],
    reportFollowUps: [
      "建议为高频追问准备量化数据（容量、延迟、成本）",
      "建议把一次线上问题的复盘整理成自己的方法论",
    ],
    reportLimitations: ["演示环境未连接真实模型，评语为脚本内容，仅供参考"],
  },
  misc: {
    demoScriptTitle: "演示讲稿",
    meetingTitle: "演示会议室（虚构） | Microsoft Teams",
    speakerName: "扬声器（演示设备）",
    virtualAudioReady: "虚拟声卡就绪（演示环境，未安装任何驱动）。",
    virtualAudioNoInstall: "演示环境无需安装驱动，虚拟声卡保持就绪。",
    notImplemented: (cmd) => `在线演示尚未模拟该能力（${cmd}）`,
  },
  livestream: {
    inputInvalid: "请输入产品标题并选择至少一份资料。",
    noMaterial: "所选资料没有可用的内容小节。",
    notConfirmed: "请先确认讲稿再开始播放。",
    unknownAction: (action) => `未知的直播控制动作（${action}）`,
    questionEmpty: "请输入人工提问。",
    questionSegmentTitle: (question) => `回答提问：${question.slice(0, 12)}`,
    questionSegmentText: (question) =>
      `关于「${question}」：依据已确认讲稿与资料，直播讲解员只陈述资料中写明的内容，不做额外承诺。具体数据与口径以资料为准。`,
  },
};

const EN: DemoText = {
  roles: {
    nameRequired: "Role name cannot be empty.",
    copySourceMissing: "Role to duplicate not found.",
    copySuffix: " (copy)",
    notFound: "Role not found.",
  },
  providers: {
    endpointRequired: "Endpoint URL cannot be empty.",
    notFound: "Provider not found.",
    inUse: "The provider is still referenced by a voice route or an Embedding configuration.",
    embeddingNotFound: "Embedding configuration not found.",
    statusConfigured: "Configured (demo)",
    modelSuffix: " (demo)",
  },
  voice: {
    routeNameRequired: "Route name cannot be empty.",
    routeNotFound: "Voice route not found.",
    routeNotReady: "The route has not passed its test yet and cannot be enabled.",
    voiceNotFound: "Voice not found.",
    statusConfigured: "Configured (demo)",
  },
  materials: {
    pathRequired: "Enter a material file path.",
    importSectionTitle: "Imported content",
    importSectionText: (fileName) =>
      `Demo material "${fileName}". The online demo never reads local file contents; this placeholder text exists only for the interface. The desktop app chunks, embeds, and full-text searches real files.`,
  },
  records: {
    notFound: "Session record not found.",
    markdown: {
      sessionHeading: (id) => `Session record ${id}`,
      role: "Role",
      status: "Status",
      started: "Started",
      finished: "Finished",
      transport: "Transport",
      disclaimer: "Online demo data. All content is fictional.",
      turnHeading: (n) => `Turn ${n}`,
      user: "User",
      assistant: "Assistant",
      citations: "Citations:",
    },
    downloaded: (fileName) => `Downloaded via the browser: ${fileName} (the online demo never writes to local disk)`,
  },
  live: {
    noActiveSession: "No session is currently running.",
    retryFallback: "(Retry) Sure, let's go through it again.",
    tailReplies: [
      "That's the end of the demo script. The desktop app keeps the voice conversation going; in the online demo you can keep typing, or end the session and review the records.",
      "This is the demo's fixed ending: replay the full script by ending and restarting the session; for real voice, download the desktop app.",
    ],
    reportFallbackRole: "Demo role",
    reportSummary: (turns, role) =>
      `This scenario ran ${turns} turns (role: ${role}). Online demo scores come from a fixed script and do not represent real model evaluation; the desktop app generates score reports from real conversations.`,
    reportStrengths: [
      "Clear boundaries around core concepts, grounded in component-level solutions",
      "Gives trade-off reasoning under follow-up questions instead of bare conclusions",
    ],
    reportFollowUps: [
      "Prepare quantitative data (capacity, latency, cost) for frequent follow-ups",
      "Turn one incident retrospective into your own repeatable method",
    ],
    reportLimitations: ["The demo environment has no real model attached; notes are scripted and for reference only"],
  },
  misc: {
    demoScriptTitle: "Demo script",
    meetingTitle: "Demo meeting room (fictional) | Microsoft Teams",
    speakerName: "Speakers (demo device)",
    virtualAudioReady: "Virtual audio device ready (demo environment; no driver installed).",
    virtualAudioNoInstall: "The demo environment needs no driver; the virtual audio device stays ready.",
    notImplemented: (cmd) => `The online demo has not simulated this capability (${cmd})`,
  },
  livestream: {
    inputInvalid: "Enter a product title and select at least one material.",
    noMaterial: "The selected materials have no usable sections.",
    notConfirmed: "Confirm the script before starting playback.",
    unknownAction: (action) => `Unknown live control action (${action})`,
    questionEmpty: "Enter a question first.",
    questionSegmentTitle: (question) => `Answering: ${question.slice(0, 24)}`,
    questionSegmentText: (question) =>
      `About "${question}": per the confirmed script and materials, the presenter only states what the materials say and makes no extra promises. Specifics defer to the materials.`,
  },
};

const TEXT: Record<Language, DemoText> = { "zh-CN": ZH, en: EN };

/** 按当前界面语言取演示文案表；种子与动态回复共用。 */
export function demoT(): DemoText {
  return TEXT[demoLang()];
}
