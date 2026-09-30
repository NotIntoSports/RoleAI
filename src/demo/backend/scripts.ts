// 演示脚本：3 套虚构的“实时模拟面试/点评/纪要”对话时间线素材。
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

import { COACH_SCRIPT_EN, INTERVIEW_SCRIPT_EN, MEETING_SCRIPT_EN } from "./scripts-en";

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

const ALL_SCRIPTS: Record<Language, Record<string, DemoScript>> = {
  "zh-CN": { [INTERVIEW_SCRIPT.id]: INTERVIEW_SCRIPT, [COACH_SCRIPT.id]: COACH_SCRIPT, [MEETING_SCRIPT.id]: MEETING_SCRIPT },
  en: { [INTERVIEW_SCRIPT_EN.id]: INTERVIEW_SCRIPT_EN, [COACH_SCRIPT_EN.id]: COACH_SCRIPT_EN, [MEETING_SCRIPT_EN.id]: MEETING_SCRIPT_EN },
};

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
  return scripts["interview-strict"];
}
