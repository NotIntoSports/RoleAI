// 演示会话记录种子：4 条完全虚构的历史会话（2 场模拟面试、1 场会议纪要、1 场直播排练）。
import type { TurnLatencyView, TurnTimeline } from "../../generated/bindings";

export interface DemoSeedTurn {
  user: string;
  assistant: string;
  materialsUsed?: boolean;
  citations?: Array<{ materialId: string; snippet: string }>;
}

export interface DemoSeedSession {
  id: string;
  roleProfileId: string;
  status: string;
  transportMode: string;
  startedAt: string;
  finishedAt: string;
  turns: DemoSeedTurn[];
}

const ROUTE = "route-demo-realtime";

/**
 * 种子轮次的虚构分阶段时间线（数值与演示横幅「数据为虚构」声明一致）：
 * - realtime 总延迟（responseCreated → firstAudio）0.4～0.9s；
 * - cascade 全链路（asrDone → ttsDone）1.2～2.5s。
 * 由 (会话序, 轮序) 确定性生成，保证每次播种数据一致、截图可复现。
 */
export function seedTurnLatency(
  transportMode: string,
  sessionIndex: number,
  turnIndex: number,
): TurnLatencyView {
  const seed = sessionIndex * 31 + turnIndex * 7;
  const pick = (min: number, max: number) => min + ((seed * 13 + min) % (max - min));
  const timeline: TurnTimeline =
    transportMode === "realtime-e2e"
      ? {
          speechStartedMs: null,
          speechStoppedMs: null,
          transcriptDoneMs: pick(60, 140),
          responseCreatedMs: pick(150, 230),
          firstAudioMs: pick(430, 900),
          responseDoneMs: pick(1_600, 4_200),
          asrDoneMs: null,
          retrievalDoneMs: null,
          llmFirstTokenMs: null,
          llmDoneMs: null,
          ttsDoneMs: null,
          playbackStartedMs: null,
          playbackDoneMs: null,
        }
      : {
          speechStartedMs: null,
          speechStoppedMs: null,
          transcriptDoneMs: null,
          responseCreatedMs: null,
          firstAudioMs: null,
          responseDoneMs: null,
          asrDoneMs: pick(280, 620),
          retrievalDoneMs: pick(640, 860),
          llmFirstTokenMs: pick(900, 1_400),
          llmDoneMs: pick(1_500, 2_200),
          ttsDoneMs: pick(2_260, 2_500),
          playbackStartedMs: null,
          playbackDoneMs: null,
        };
  return { routeId: ROUTE, mode: transportMode === "realtime-e2e" ? "realtime" : "cascade", interrupted: false, timeline };
}

export const DEMO_SESSIONS: DemoSeedSession[] = [
  {
    id: "session-demo-interview-backend",
    roleProfileId: "preset-strict-interviewer",
    status: "completed",
    transportMode: "realtime-e2e",
    startedAt: "2026-09-27T19:30:00+08:00",
    finishedAt: "2026-09-27T19:58:00+08:00",
    turns: [
      {
        user: "面试官您好。我在云帆科技负责协同办公平台的消息服务，日活大约四十万，主要技术栈是 Java、Redis 和 Kafka。",
        assistant:
          "介绍收到。先从基础追问：你们消息服务的已读回执是怎么存储的？为什么选这个方案，而不是直接在数据库里存一条已读记录？",
      },
      {
        user: "已读回执我们用 Redis 的 Bitmap，按会话 ID 分桶，每个成员对应一位，一个人一天最多占几个字节，比行存省得多。",
        assistant:
          "Bitmap 按位存储确实省内存。那追问一句：成员退出群之后，这个桶里的位并不会收缩，长期累积会不会有内存浪费？你们怎么处理？",
      },
      {
        user: "桶有空闲淘汰策略，三十天没有写入就整体删除，删除前把已读进度归档到 MySQL，成员再进来时从归档恢复。",
        assistant:
          "归档加整体删除，粒度是能对上的。接下来看缓存：你们的资料库查询加了 Redis 缓存。如果某个热点资料恰好过期，瞬间上千个请求同时回源数据库，会发生什么？你怎么处理？",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-article", snippet: "缓存击穿指单个热点 key 过期的瞬间，大量并发请求同时回源数据库……" },
        ],
      },
      {
        user: "这就是缓存击穿。我们做了两层：热点 key 用逻辑过期，物理上不过期；发现逻辑过期后用互斥锁重建，只放一个请求回源，其他请求先拿旧值。",
        assistant:
          "逻辑过期加单飞重建是标准做法。那我要挑个刺：如果拿到锁的重建线程挂了，或者回源特别慢，这把锁没有超时会怎样？你们的锁是怎么加的？",
      },
      {
        user: "用的是 Redisson 的分布式锁，看门狗会自动续期，不会出现锁永远不释放的死锁；重建失败也有重试上限，超过就放旧值降级。",
        assistant:
          "看门狗续期这一点你说得准确。再往下：穿透和雪崩你们分别怎么防？和击穿放在一起，给我一个完整的分层方案，别说散点。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-article", snippet: "缓存穿透指查询根本不存在的数据，请求每次都打到数据库。防御手段：布隆过滤器……" },
        ],
      },
      {
        user: "穿透在入口加布隆过滤器，查不到的 key 直接拒绝，同时空值也缓存六十秒；雪崩靠过期时间加随机抖动，再加本地一级缓存，回源链路有限流兜底。",
        assistant:
          "分层是清晰的：入口拦截、空值缓存、TTL 抖动、多级缓存、限流降级，每一层都有明确的职责。最后一个追问：如果 DBA 告诉你主从延迟三秒，你的缓存一致性方案哪一层最先出问题？为什么？",
      },
      {
        user: "最先出问题的应该是“更新库后删缓存”的窗口：从库还没同步，别的请求读到从库旧数据又把它回填进缓存。我们会改成订阅 binlog 异步删缓存，延迟期间靠短 TTL 兜底。",
        assistant:
          "思路对：把强一致降级为最终一致，用 TTL 兜底，并且你点出了“旧值回填”这个真正的坑。今天的后端部分到这里，留一个思考题：如果日活涨十倍，你的 Bitmap 桶策略哪一点最先失效？下次从这道题继续。",
      },
      {
        user: "我猜是位图的粒度问题：位不能按成员淘汰，只能整桶删除，桶越大浪费越多，可能要改成按成员分片的小桶或者改用 HyperLogLog 加精确补查。",
        assistant:
          "对，位图的粒度是位不是人，这是它天然的边界。本轮评价：Redis 数据结构选型（优）、缓存击穿/穿透/雪崩分层（优）、一致性边界主动思考（良）。方案都能落到具体组件，取舍讲得清楚。回去等下一轮安排，建议补一补 Kafka 顺序性问题。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-jd", snippet: "熟悉 Redis、MySQL、Kafka 等常用组件的原理与调优……" },
        ],
      },
    ],
  },
  {
    id: "session-demo-interview-project",
    roleProfileId: "preset-strict-interviewer",
    status: "completed",
    transportMode: "realtime-e2e",
    startedAt: "2026-09-28T20:00:00+08:00",
    finishedAt: "2026-09-28T20:26:00+08:00",
    turns: [
      {
        user: "上一轮之后我补了 Kafka 的知识。这次想请您重点追问简历里的秒杀项目。",
        assistant:
          "可以，就聊秒杀。先说场景：一万件库存，瞬时二十万请求，你的入口层怎么扛？先讲限流算法的选型和理由。",
      },
      {
        user: "入口用网关层令牌桶，按用户 ID 和 IP 两维度限流；再往下单机用滑动窗口计数兜底，防止突发毛刺。",
        assistant:
          "令牌桶应对突发、滑动窗口防毛刺，这个组合说得过去。追问：你的令牌桶速率是怎么定的？拍脑袋还是有依据？如果运营临时把活动提前十分钟，你的限流参数来得及改吗？",
      },
      {
        user: "速率按压测容量的八成配置，存在配置中心，改了之后秒级生效；应急也有总开关可以直接把活动切到排队页。",
        assistant:
          "有压测依据和配置中心，这个回答是及格线以上的。下一题：库存不能超卖，一千个请求同时扣最后一件库存，你的扣减逻辑怎么写？",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-jd", snippet: "理解分布式系统常见问题（一致性、幂等、限流、降级）……" },
        ],
      },
      {
        user: "库存预热到 Redis，用 Lua 脚本把“读库存、判断、扣减、写回”做成原子操作，扣减成功才发 MQ 去落库，数据库层还有版本号乐观锁兜底。",
        assistant:
          "Redis 原子扣减加 MQ 异步落库加乐观锁兜底，层次是对的。那 Redis 和数据库之间的最终一致怎么保证？MQ 消费失败或者服务在中间宕机，订单和库存对不上账怎么办？",
      },
      {
        user: "消费失败有重试队列，超过次数进死信人工处理；对账有定时任务，每五分钟对一次 Redis 扣减流水和订单表，差异自动补偿。",
        assistant:
          "对账补偿能想到，说明你处理过真实问题。继续：同一个用户重复点击或者网络重试，同一件商品会不会生成两笔订单？你的幂等方案是什么？",
      },
      {
        user: "幂等键是用户 ID 加活动 ID，落库用唯一索引防重，请求进来先查 Redis 的去重集合，命中直接返回上一次的结果。",
        assistant:
          "唯一索引是最后防线，放在数据库层而不是只靠 Redis 去重，这点很关键，你答对了。最后一个问题：二十万请求进来，下游数据库写入只有五千 TPS 的能力，MQ 堆积了怎么办？",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-article", snippet: "对回源链路做限流与降级预案……" },
        ],
      },
      {
        user: "活动期间消费端只做最轻的落库，非核心链路（积分、通知）异步化或者直接降级关闭；堆积超过水位就扩消费者分区，活动结束前保证消化完。",
        assistant:
          "核心链路保住、非核心链路让路，这个优先级是对的。本轮评价：限流选型（优）、库存原子扣减（优）、幂等与对账（优）、容量与降级预案（良）。两轮整体看，你对真实系统的边界是有感觉的，建议把“对账补偿的自动化程度”再往下想一层。",
      },
      {
        user: "谢谢面试官，对账自动化这块我回去会再整理一版方案。",
        assistant:
          "不客气。把每次追问都变成你的清单，这就是面试训练的意义。结束前提醒：你的表达节奏不错，但遇到追问时语速会明显加快，注意停顿，给思考留白。下一轮我们约系统设计。",
      },
    ],
  },
  {
    id: "session-demo-meeting-notes",
    roleProfileId: "preset-meeting",
    status: "completed",
    transportMode: "meeting-bridge",
    startedAt: "2026-09-29T10:00:00+08:00",
    finishedAt: "2026-09-29T10:47:00+08:00",
    turns: [
      {
        user: "开始记录产品评审会。今天的议题是云帆平台 v2.3 的上线范围。",
        assistant: "好的，已在记录会议内容。议题：v2.3 上线范围。需要我回答问题时请@会议助手。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3（2026-09）新增项目空间与离线同步改进……" },
        ],
      },
      {
        user: "@会议助手 v2.3 相比 v2.2 的客户端崩溃率数据是多少？",
        assistant:
          "依据产品手册：v2.3 客户端崩溃率（虚构指标）为 0.12%，较 v2.2 下降约 40%。此数据来自手册“安全与合规”章节，建议以监控平台当日数据为准。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3 的客户端崩溃率（虚构指标）为 0.12%，较 v2.2 下降约 40%。" },
        ],
      },
      {
        user: "@会议助手 离线模式下文档编辑有什么限制？",
        assistant:
          "手册“版本记录”章节写明：离线模式下文档编辑暂不支持表格函数自动重算，其余编辑能力可用。是否影响本次上线范围，建议与文档团队确认。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "已知限制：离线模式下文档编辑暂不支持表格函数自动重算。" },
        ],
      },
      {
        user: "我们再过一下风险清单：离线同步改造涉及三端，回滚方案是保留 v2.2 的同步通道两周。",
        assistant: "已记录：离线同步改造涉及三端；回滚方案为保留 v2.2 同步通道两周。",
      },
      {
        user: "@会议助手 把今天的结论整理成三句话。",
        assistant:
          "今日结论：一、v2.3 按期上线，范围包含项目空间与离线同步改进；二、崩溃率 0.12%（较 v2.2 下降约 40%，虚构演示数据）；三、回滚方案为保留 v2.2 同步通道两周。事实项以上传资料为准，未确认事项已标注。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3（2026-09）新增项目空间与离线同步改进；v2.2（2026-06）新增会议助手与审计导出……" },
        ],
      },
      {
        user: "散会前确认：审计日志导出功能是谁负责验收？",
        assistant: "本轮发言未包含验收人信息，已原样记录：“审计日志导出功能验收人待确认”。",
      },
      {
        user: "补充一下日程：下周二走全量灰度，周四复盘灰度数据。",
        assistant: "已记录日程：下周二全量灰度；周四复盘灰度数据。",
      },
      {
        user: "@会议助手 审计日志能保留多久、能不能导给外部平台？",
        assistant:
          "依据产品手册：审计日志保留 365 天，支持导出给 SIEM 平台。以上为手册“安全与合规”章节内容，具体交付口径建议以安全团队确认为准。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "审计日志保留 365 天，支持导出给 SIEM 平台。" },
        ],
      },
    ],
  },
  {
    id: "session-demo-livestream",
    roleProfileId: "preset-presenter",
    status: "completed",
    transportMode: "realtime-e2e",
    startedAt: "2026-09-26T15:00:00+08:00",
    finishedAt: "2026-09-26T15:22:00+08:00",
    turns: [
      {
        user: "开始直播排练，今天讲解云帆协同办公平台，资料用上传的产品手册。",
        assistant:
          "排练开始。讲稿将严格依据《云帆协同办公平台产品手册 v2.3》生成，不编造价格、库存与优惠信息。第一段：开场介绍。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "云帆协同办公平台（虚构产品）面向 50～5000 人规模的企业……" },
        ],
      },
      {
        user: "开场怎么说？给我一段三十秒的口播。",
        assistant:
          "“大家好，今天带大家看云帆协同办公平台：消息、文档、日程、审批，一个平台搞定。五十人到五千人的团队都能用，Windows、macOS 和浏览器都有客户端。今天重点演示 v2.3 新上的项目空间和离线同步。”（约三十秒）",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3 重点加强了跨部门项目空间与离线消息同步……" },
        ],
      },
      {
        user: "弹幕有人问：离线消息保留多久？",
        assistant:
          "回答弹幕：依据手册，离线消息在服务端保留九十天，重新上线后按会话增量拉取。只讲手册里写明的，不额外承诺。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "离线消息在服务端保留 90 天，重新上线后按会话增量拉取。" },
        ],
      },
      {
        user: "再讲一下文档协同的卖点。",
        assistant:
          "“文档这块，多人实时编辑不冲突，历史版本保留一百八十天，改错了随时回溯。表格支持常见函数和数据透视。文档还能一键转评审任务，意见自动汇总到项目空间。”",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "在线文档支持多人实时编辑与历史版本回溯，历史版本保留 180 天……" },
        ],
      },
      {
        user: "有人问价格，怎么说？",
        assistant:
          "价格问题按合规口径回避：“价格和优惠以官网和商务为准，直播间不报价，需要的话联系商务获取报价单。”手册未包含价格信息，讲解员不编造。",
      },
      {
        user: "安全合规这块怎么讲？",
        assistant:
          "“安全方面多说一句：全链路 TLS 1.3，租户数据隔离加密，管理员可以配水印、外发审批和设备白名单，审计日志保留一整年，还能对接企业自己的审计平台。”",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "传输层全链路 TLS 1.3，存储层按租户隔离加密……" },
        ],
      },
      {
        user: "弹幕问审计日志怎么对接他们公司平台。",
        assistant:
          "回答弹幕：手册写明审计日志保留 365 天、支持导出给 SIEM 平台；具体对接方式建议留下联系方式，由解决方案同事提供对接文档，直播间不展开技术细节。",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "审计日志保留 365 天，支持导出给 SIEM 平台。" },
        ],
      },
      {
        user: "最后做个收尾。",
        assistant:
          "“总结一下：跨部门项目空间、离线同步、审计合规，这是 v2.3 的三个关键词。感兴趣的同学可以从官网下载客户端体验，我们下期直播看文档协同的实战。”",
      },
    ],
  },
];

import { currentLanguage, type Language } from "../../i18n";

import { DEMO_SESSIONS_EN } from "./records-data-en";

const ALL_SESSIONS: Record<Language, DemoSeedSession[]> = { "zh-CN": DEMO_SESSIONS, en: DEMO_SESSIONS_EN };

/** 按当前界面语言取会话种子；数值时间线与语言无关，仍由 seedTurnLatency 生成。 */
export function demoSessions(): DemoSeedSession[] {
  return ALL_SESSIONS[currentLanguage()];
}
