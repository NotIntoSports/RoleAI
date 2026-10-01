// 演示脚本：3 套虚构的“实时面试官对练/点评/纪要”对话时间线素材。
// 全部内容为演示数据，不指向任何真实人物或公司。
export interface DemoScriptTurn {
  /** 用户发言（演示里以假 ASR 逐字上屏）。 */
  userText: string;
  /** AI 回答（流式上屏，可选浏览器语音朗读）。 */
  replyText: string;
  /** 需要演示“用户打断”的轮：回答在该字符数处被截断。 */
  interruptAfterChars?: number;
  materialsUsed?: boolean;
  citations?: Array<{ materialId: string; snippet: string }>;
}

export interface DemoScript {
  id: string;
  turns: DemoScriptTurn[];
}

import { currentLanguage, type Language } from "../../i18n";

import {
  COACH_SCRIPT_EN,
  CS_DRILL_SCRIPT_EN,
  DEBATE_SCRIPT_EN,
  ENGLISH_SCRIPT_EN,
  INTERVIEW_SCRIPT_EN,
  MEETING_SCRIPT_EN,
  MOCK_CANDIDATE_SCRIPT_EN,
} from "./scripts-en";

const HANDBOOK = "mat-demo-handbook";

/** 严苛面试官追问（第 3 轮演示打断）。 */
export const INTERVIEW_SCRIPT: DemoScript = {
  id: "interview-strict",
  turns: [
    {
      userText:
        "面试官您好，我最有代表性的项目是协同办公平台的消息服务，日活大约四十万，技术栈是 Java 加 Redis。",
      replyText:
        "好，项目记下了。我先追问细节：消息的已读回执你们是怎么存储的？为什么选这个方案，而不是直接在数据库里放一张已读表？内存账怎么算的？",
    },
    {
      userText:
        "已读回执我们用 Redis 的 Bitmap，按会话 ID 分桶，每个成员占一位，一个人一天最多几个字节；桶三十天没写入就整体删除，删除前归档到 MySQL。",
      replyText:
        "分桶加空闲淘汰，粒度能对上。接下来看缓存：假设一份热点资料恰好过期，瞬间上千个请求同时回源数据库，这就是缓存击穿——你们会怎么处理？",
    },
    {
      userText:
        "我们用逻辑过期加互斥重建：key 物理上不过期，发现逻辑过期后只放一个请求持锁回源，其他请求先拿旧值，锁用 Redisson，看门狗自动续期。",
      replyText:
        "逻辑过期加单飞重建，是标准做法。我再往细节里压一压：如果持锁的重建线程特别慢，等旧值的请求越积越多，超时层和用户体验层怎么设计？我给你拆三块：先说锁的粒度选择，再说等待方的超时策略，最后……",
      interruptAfterChars: 52,
    },
    {
      userText:
        "不好意思打断一下，锁粒度这块我们恰好做过一轮优化，正好可以用数据说明，我讲一下这个案例可以吗？",
      replyText:
        "可以，你说。顺便提醒：面试里主动打断并给出自己的论据，本身就是加分行为，说明你对方案有自己的数据。讲吧，我按你的案例继续追问。",
    },
  ],
};

/** 表达教练点评。 */
export const COACH_SCRIPT: DemoScript = {
  id: "coach",
  turns: [
    {
      userText:
        "嗯……我们的项目主要是做一个消息服务，然后，嗯，日活大概四十万，技术用的主要是 Java，然后 Redis 也是。",
      replyText:
        "先说最影响效果的三个问题。第一，口头禅：“嗯”“然后”一共出现四次，削弱专业感。第二，信息散：项目、规模、技术栈三个要点缠在一起。第三，没有落点，听的人抓不到重心。改写示范：“我负责协同办公平台的消息服务，支撑日活四十万，技术栈 Java 加 Redis。”理由：结论先行、数字单独成句、删掉全部填充词。",
    },
    {
      userText: "我再试一遍：我负责协同办公平台的消息服务，支撑日活四十万，技术栈 Java 加 Redis。",
      replyText:
        "好很多。再给一个小的优化点：把“日活四十万”放到句尾停顿半拍再说，数字会更有分量：“我负责协同办公平台的消息服务——日活，四十万。”这一轮我们聚焦的是口头禅，下一轮可以练逻辑连接词。",
    },
    {
      userText: "明白了。那三分钟的自我介绍应该怎么搭结构？",
      replyText:
        "三分钟建议四段式：一句定位（我是谁、几年经验）、一个代表成果（带数字）、一段能力证明（具体案例）、一句岗位匹配（为什么是这个岗位）。每段约四十秒。练法：开手机录音讲三遍，每遍至少删掉一个“然后”，讲完自己听一遍再改。",
    },
  ],
};

/** 会议助手纪要（点名提问场景）。 */
export const MEETING_SCRIPT: DemoScript = {
  id: "meeting",
  turns: [
    {
      userText: "@会议助手 今天的会议结论有哪些？帮我纪要一下。",
      replyText:
        "本次会议要点：议题围绕产品 v2.3 的上线范围与风险。已确认事实（依据上传的产品手册）：v2.3 新增跨部门项目空间与离线同步改进。待确认事项：灰度节奏与回滚窗口，本轮发言中尚未提及。",
      materialsUsed: true,
      citations: [
        { materialId: HANDBOOK, snippet: "v2.3 重点加强了跨部门项目空间与离线消息同步……" },
      ],
    },
    {
      userText: "@会议助手 线上崩溃率现在是多少？和上一版比呢？",
      replyText:
        "产品手册“安全与合规”章节记录：v2.3 客户端崩溃率（虚构指标）为 0.12%，较 v2.2 下降约 40%。建议以监控平台当日数据为准，手册数字可能滞后。",
      materialsUsed: true,
      citations: [
        { materialId: HANDBOOK, snippet: "v2.3 的客户端崩溃率（虚构指标）为 0.12%，较 v2.2 下降约 40%。" },
      ],
    },
    {
      userText: "@会议助手 把今天的内容整理成三句话的结论。",
      replyText:
        "今日结论三句话：一、v2.3 按期上线，范围是项目空间与离线同步；二、崩溃率 0.12%、较上版下降约 40%（虚构演示数据）；三、灰度与回滚口径待下一次会议确认。事实项均以上传资料为准。",
      materialsUsed: true,
      citations: [
        { materialId: HANDBOOK, snippet: "v2.3（2026-09）新增项目空间与离线同步改进……" },
      ],
    },
  ],
};

/** 辩论陪练：站对立面攻防（辩题“远程办公优于集中办公”为虚构示例）。 */
export const DEBATE_SCRIPT: DemoScript = {
  id: "debate",
  turns: [
    {
      userText:
        "我方立场：远程办公优于集中办公，因为它省掉了通勤时间，让员工把更多精力投入工作和生活。",
      replyText:
        "我方反对。通勤省下的时间并不自动变成工作产出——居家环境里专注力和协作效率同时下降，新人成长尤其吃亏。请正面回答：没有当面协作，跨部门决策变慢的问题你们怎么解决？",
    },
    {
      userText:
        "决策变慢可以用异步协作工具解决：文档先行、结论留痕，反而比口头拍板更透明。",
      replyText:
        "“透明”是写下来的幻觉：文档先行会把快速对齐拖成文字拉锯，关键分歧往往拖到截止日前才爆发。再想想——异步工具解决的是“记录”，不是“共识”，这两件事不要混为一谈。",
    },
    {
      userText: "那我们折中：核心协作日到岗，其余时间远程，兼顾专注与协作。",
      replyText:
        "折中听着稳妥，但“混合”意味着两套规则的维护成本：会议室和排期都要迁就能到岗的人，公平性反而更难保证。注意，你的论证已经从“优于”退到了“不差于”——这一步退让本身就是我方的得分点。",
    },
  ],
};

/** 英语口语陪练：英文对话 + 单点中文纠错。 */
export const ENGLISH_SCRIPT: DemoScript = {
  id: "english-partner",
  turns: [
    {
      userText: "I have go to a meeting every Monday, and I always feel nervous to speak English in the meeting.",
      replyText:
        "That's completely normal — most people feel that way at first. 顺便纠一个错：“I have go”要说“I have to go”；“nervous to speak”更自然的说法是“nervous about speaking”。再来一句：这周的会议里，你想表达什么观点？",
    },
    {
      userText: "I want to share my opinion about the new design, but I'm afraid my grammar is not well.",
      replyText:
        "Great — that's exactly the kind of sentence you need in meetings. 只有一个小问题：“not well”应该是“not good”，grammar 是名词，要用形容词。更自信的开场是“I'd like to share my thoughts on the new design.”试着把它完整说一遍？",
    },
    {
      userText: "I would like to share my thoughts on the new design. I think the navigation bar should be more simple.",
      replyText:
        "Perfect — natural and confident. 一个小优化：“more simple”说成“simpler”更口语。今天你已经能完整表达一个工作观点了，下次我们练“礼貌地表达不同意”，好不好？",
    },
  ],
};

/** 客服演练：AI 扮演难缠客户，演示“投诉→安抚→和解”的完整弧线。 */
export const CS_DRILL_SCRIPT: DemoScript = {
  id: "cs-drill",
  turns: [
    {
      userText:
        "非常抱歉给您带来不便。我先帮您查一下订单的最新物流状态，麻烦您提供一下订单号。",
      replyText:
        "订单号我都背下来了！每次打电话都是这一套：报单号、查询，然后“正在处理”。我不要听“正在处理”，我要一个准确的送达日期。你到底查得到还是查不到？",
    },
    {
      userText:
        "查到了，包裹卡在中转站。这样，我马上为您申请加急派送，同时因为延误十天，补偿您一张五十元无门槛优惠券，今天到账。",
      replyText:
        "加急？上次你们也是这么说的！优惠券我不稀罕。这样，你能保证后天之内送到，这事就算了；保证不了，我现在就退货退款，运费你们全出。你敢不敢给句准话？",
    },
    {
      userText:
        "我把“后天 24 点前送达”写进工单并给您发短信确认。如果超时，退货退款、运费全免，另外再补偿一张券。您看这样处理可以吗？",
      replyText:
        "……行，就按你说的办，白纸黑字写进工单。说句公道话，你今天态度还可以，是前面的客服把我耗没脾气的。记得，短信一定要发！",
    },
  ],
};

/** 模拟应聘者：用户当面试官，应聘者回答刻意留有可追问的模糊点。 */
export const MOCK_CANDIDATE_SCRIPT: DemoScript = {
  id: "mock-candidate",
  turns: [
    {
      userText: "小林你好，先介绍一下你做过的最有代表性的项目。",
      replyText:
        "好的。我最近一年在做一个内部的数据同步平台，日同步量大概千万级，我负责失败重试和监控告警模块。整体上……呃，主要就是保证了数据不丢。具体架构是我导师搭的，我参与了不少模块。",
    },
    {
      userText: "“保证了数据不丢”具体是怎么做到的？重试策略是什么，会不会造成重复写入？",
      replyText:
        "重试用的是指数退避，最多三次。重复写入的问题……呃，下游有做幂等，用业务 ID 去重。可能确实有极小概率丢数据，但我们的场景允许偶尔补录，所以当时没有上更强的一致性方案。",
    },
    {
      userText: "如果让你重新设计，你会怎么改进这个方案？",
      replyText:
        "我会把重试和幂等收敛到一个独立的传输层，再加对账任务兜底，而不是散在各个业务模块里。这也是我这次看机会的原因——想去更大规模的场景，看看成熟团队是怎么处理这类问题的。",
    },
  ],
};

const ALL_SCRIPTS: Record<Language, Record<string, DemoScript>> = {
  "zh-CN": {
    [INTERVIEW_SCRIPT.id]: INTERVIEW_SCRIPT,
    [COACH_SCRIPT.id]: COACH_SCRIPT,
    [MEETING_SCRIPT.id]: MEETING_SCRIPT,
    [DEBATE_SCRIPT.id]: DEBATE_SCRIPT,
    [ENGLISH_SCRIPT.id]: ENGLISH_SCRIPT,
    [CS_DRILL_SCRIPT.id]: CS_DRILL_SCRIPT,
    [MOCK_CANDIDATE_SCRIPT.id]: MOCK_CANDIDATE_SCRIPT,
  },
  en: {
    [INTERVIEW_SCRIPT_EN.id]: INTERVIEW_SCRIPT_EN,
    [COACH_SCRIPT_EN.id]: COACH_SCRIPT_EN,
    [MEETING_SCRIPT_EN.id]: MEETING_SCRIPT_EN,
    [DEBATE_SCRIPT_EN.id]: DEBATE_SCRIPT_EN,
    [ENGLISH_SCRIPT_EN.id]: ENGLISH_SCRIPT_EN,
    [CS_DRILL_SCRIPT_EN.id]: CS_DRILL_SCRIPT_EN,
    [MOCK_CANDIDATE_SCRIPT_EN.id]: MOCK_CANDIDATE_SCRIPT_EN,
  },
};

/** 全部演示脚本 id（与语言无关；live-commands 用它做白名单）。 */
export const DEMO_SCRIPT_IDS: ReadonlySet<string> = new Set(
  Object.values(ALL_SCRIPTS).flatMap((scripts) => Object.keys(scripts)),
);

/** 按脚本 id 取当前语言的脚本（演示回退与 live-commands 的显式选择共用）。 */
export function demoScriptById(id: string): DemoScript {
  const scripts = ALL_SCRIPTS[currentLanguage()];
  return scripts[id] ?? scripts["interview-strict"];
}

/** 脚本播完后的兜底回答（循环使用；语言相关文案走 demoT）。 */
export function scriptForRole(roleProfileId: string | null, scenario: string | undefined): DemoScript {
  const scripts = ALL_SCRIPTS[currentLanguage()];
  if (roleProfileId === "preset-expression-coach") return scripts["coach"];
  if (roleProfileId === "preset-meeting" || scenario === "meetingAssistant") return scripts["meeting"];
  if (roleProfileId === "preset-presenter" || scenario === "livestreamPresenter") {
    // 直播讲解员复用会议脚本（讲稿口播场景）。
    return scripts["meeting"];
  }
  if (roleProfileId === "preset-debate-partner") return scripts["debate"];
  if (roleProfileId === "preset-english-partner") return scripts["english-partner"];
  if (roleProfileId === "preset-cs-drill") return scripts["cs-drill"];
  if (roleProfileId === "preset-mock-candidate") return scripts["mock-candidate"];
  return scripts["interview-strict"];
}
