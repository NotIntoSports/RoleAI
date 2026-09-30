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
    triggerAssistant: string;
    retryFallback: string;
    tailReplies: string[];
    reportFallbackRole: string;
    reportSummary: (turns: number, role: string) => string;
    reportStrengths: string[];
    reportFollowUps: string[];
    reportLimitations: string[];
  };
  practice: {
    planInvalid: string;
    planNotFound: string;
    sessionStateInvalid: string;
    noTranscripts: string;
    reportNotFound: string;
    defaultPosition: string;
    defaultStyle: string;
    strictStyle: string;
    defaultDifficulty: string;
    questionFallback: (n: number) => string;
    modelAnswerDemo: string;
    generateModelAnswer: string;
    generateStrengthsFirst: string[];
    generateStrengthsRest: string[];
    generateIssuesFirst: string[];
    generateIssuesRest: string[];
    generateSuggestions: string[];
    seedSuggestionsWeak: string[];
    seedSuggestionsStrong: string[];
    seedModelAnswer: string;
    seedAnswerWeak: string;
    seedAnswerStrong: string;
    seedStrengthsWeak: string[][];
    seedStrengthsStrong: string[][];
    seedIssuesWeak: string[][];
    seedIssuesStrong: string[][];
    markdown: {
      title: (position: string) => string;
      totalScore: (score: number) => string;
      dimensions: (d: { contentDepth: number; structureClarity: number; fluency: number; jobFit: number }) => string;
      disclaimer: string;
      questionHeading: (n: number, prompt: string) => string;
      myAnswer: (answer: string) => string;
      score: (score: number) => string;
      modelAnswer: (answer: string) => string;
      aiNote: string;
    };
    downloaded: (fileName: string) => string;
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
    triggerAssistant: "@会议助手 请继续",
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
  practice: {
    planInvalid: "题单参数无效",
    planNotFound: "题单不存在",
    sessionStateInvalid: "该会话不是模拟面试训练",
    noTranscripts: "会话还没有可评价的转写",
    reportNotFound: "训练报告不存在",
    defaultPosition: "后端开发工程师",
    defaultStyle: "面试官",
    strictStyle: "严苛面试官",
    defaultDifficulty: "标准",
    questionFallback: (n) => `第 ${n} 题`,
    modelAnswerDemo: "（演示示范）建议按“结论 → 依据 → 数字”三段式回答，并把口头禅替换成停顿。",
    generateModelAnswer: "（演示示范）按“结论 → 依据 → 数字”三段式重组回答。",
    generateStrengthsFirst: ["结构清晰", "有具体例子"],
    generateStrengthsRest: ["先讲结论再展开"],
    generateIssuesFirst: ["结尾缺少量化结果"],
    generateIssuesRest: [],
    generateSuggestions: ["回答先给结论再展开细节", "每个项目准备一个量化结果", "用停顿代替口头禅"],
    seedSuggestionsWeak: ["回答先给结论再展开细节", "控制口头禅，用停顿代替", "每个项目准备一个量化结果"],
    seedSuggestionsStrong: ["继续补充跨团队协作的例子", "对追问保持追问式的反问练习", "把语速稳定在 220 字/分钟以内"],    seedModelAnswer: "（演示示范）建议按“结论 → 依据 → 数字”三段式回答，并把口头禅替换成停顿。",
    seedAnswerWeak: "（演示转写）当时的情况比较复杂，主要是我负责的部分，具体细节就是……那个……做了一些优化。",
    seedAnswerStrong: "（演示转写）项目背景是接口性能不达标，我先用压测定位瓶颈，再把缓存粒度细化，最终 p99 下降 30%。",
    seedStrengthsWeak: [["态度认真"], ["思路清楚"], ["态度认真"]],
    seedStrengthsStrong: [["先讲结论再展开"], ["有量化结果"], ["结构完整"]],
    seedIssuesWeak: [["口头禅较多"], ["缺少量化结果"], ["展开没有重点"]],
    seedIssuesStrong: [[], ["时间分配可以更均衡"], []],
    markdown: {
      title: (position) => `模拟面试训练报告（${position}）`,
      totalScore: (score) => `总分：${score} / 5`,
      dimensions: (d) =>
        `维度：内容深度 ${d.contentDepth} · 结构清晰 ${d.structureClarity} · 表达流畅 ${d.fluency} · 岗位匹配 ${d.jobFit}`,
      disclaimer: "在线演示数据，全部内容均为虚构。",
      questionHeading: (n, prompt) => `第 ${n} 题：${prompt}`,
      myAnswer: (answer) => `**我的回答**：${answer}`,
      score: (score) => `**评分**：${score} / 5`,
      modelAnswer: (answer) => `**改进示范**：${answer}`,
      aiNote: "评分由 AI 生成，仅供练习参考",
    },
    downloaded: (fileName) => `已通过浏览器下载：${fileName}（在线演示不写入本地磁盘）`,
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
    triggerAssistant: "@meeting-assistant please continue",
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
  practice: {
    planInvalid: "Invalid question plan parameters",
    planNotFound: "Question plan not found",
    sessionStateInvalid: "This session is not a mock interview",
    noTranscripts: "The session has no answer transcripts to review yet",
    reportNotFound: "Training report not found",
    defaultPosition: "Backend engineer",
    defaultStyle: "Interviewer",
    strictStyle: "Strict interviewer",
    defaultDifficulty: "Standard",
    questionFallback: (n) => `Question ${n}`,
    modelAnswerDemo: "(Demo) Structure the answer as \"conclusion → reasoning → numbers\", and replace filler words with pauses.",
    generateModelAnswer: "(Demo) Rebuild the answer as \"conclusion → reasoning → numbers\".",
    generateStrengthsFirst: ["Clear structure", "Concrete examples"],
    generateStrengthsRest: ["Conclusion first, then details"],
    generateIssuesFirst: ["Ending lacks a quantified result"],
    generateIssuesRest: [],
    generateSuggestions: ["Lead with the conclusion before details", "Prepare one quantified result per project", "Use pauses instead of filler words"],
    seedSuggestionsWeak: ["Lead with the conclusion before details", "Control filler words; pause instead", "Prepare one quantified result per project"],
    seedSuggestionsStrong: ["Add more cross-team collaboration examples", "Practice reverse questions under follow-ups", "Keep speech rate under 220 characters/minute"],
    seedModelAnswer: "(Demo) Structure the answer as \"conclusion → reasoning → numbers\", and replace filler words with pauses.",
    seedAnswerWeak: "(Demo transcript) The situation was complicated — mostly my part, and the details were, you know... we did some optimizations.",
    seedAnswerStrong: "(Demo transcript) The API missed its performance target. I located the bottleneck with load testing, refined cache granularity, and cut p99 by 30%.",
    seedStrengthsWeak: [["Earnest attitude"], ["Clear thinking"], ["Earnest attitude"]],
    seedStrengthsStrong: [["Conclusion first"], ["Quantified results"], ["Complete structure"]],
    seedIssuesWeak: [["Too many filler words"], ["No quantified results"], ["Unfocused detail"]],
    seedIssuesStrong: [[], ["Time allocation could be more balanced"], []],
    markdown: {
      title: (position) => `Mock interview report (${position})`,
      totalScore: (score) => `Total score: ${score} / 5`,
      dimensions: (d) =>
        `Dimensions: content depth ${d.contentDepth} · structure clarity ${d.structureClarity} · fluency ${d.fluency} · job fit ${d.jobFit}`,
      disclaimer: "Online demo data. All content is fictional.",
      questionHeading: (n, prompt) => `Question ${n}: ${prompt}`,
      myAnswer: (answer) => `**My answer**: ${answer}`,
      score: (score) => `**Score**: ${score} / 5`,
      modelAnswer: (answer) => `**Improved example**: ${answer}`,
      aiNote: "Scores are AI generated and for practice reference only",
    },
    downloaded: (fileName) => `Downloaded via the browser: ${fileName} (the online demo never writes to local disk)`,
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
