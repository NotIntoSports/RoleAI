/**
 * OpenAI 兼容 mock 服务器（lane-I I03 桌面应用主流程 e2e 专用）。
 *
 * 仅用 node:http 内置模块实现（不新增依赖）：Rust 侧集成测试已有手写
 * HTTP mock（src-tauri/src/providers/tests.rs），Node 侧的 msw/nock 面向
 * 进程内 fetch 拦截，无法被桌面应用的外部 HTTP 客户端访问，因此按卡片
 * 要求实现最小子集，端点与 src-tauri/src/providers/cascade.rs 的
 * normalize_*_url 约定一致（baseUrl 末尾追加固定后缀）：
 *
 *   GET  {base}/models                → 模型目录（discover/test 连接测试）
 *   POST {base}/chat/completions      → LLM：题单生成 / 报告点评 / 会话轮回复
 *   POST {base}/audio/transcriptions  → ASR：{ text }（文本输入 e2e 不经过，
 *                                        仅为覆盖卡片要求的最小子集）
 *   POST {base}/audio/speech          → TTS：原始 PCM 字节（response_format=pcm，
 *                                        parse_tts_pcm 要求偶数字节）
 *
 * chat/completions 的分发依据 practice/plan.rs 与 practice/report.rs
 * 注入提示词里的固定 JSON 模板：包含 `"questions":` 视为题单生成请求，
 * 包含 `"perQuestion":` 视为点评请求，其余按普通会话轮返回固定文本。
 * 会话轮使用 stream:true 时返回的仍是普通 JSON——cascade.rs 的
 * stream_sse_text 对"全程无 SSE 帧"的响应有兼容回退（按普通补全解析）。
 *
 * /__health、/__requests 是测试辅助端点：前者用于就绪等待，后者返回
 * 已捕获的请求列表（断言"应用确实调用了 mock"的端到端证据）。
 *
 * 独立调试：node e2e/desktop/mock-openai-server.mjs [端口]（默认随机）。
 */
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

/** 普通会话轮的固定回复文本（测试断言用）。 */
export const MOCK_REPLY_TEXT = "这是来自 E2E mock 的回复：模拟面试训练闭环工作正常。";

/** 题单生成请求的固定 JSON（后端按请求题量裁剪，返回 3 道即可覆盖 1~3 题）。 */
const PLAN_JSON = {
  questions: [
    {
      prompt: "E2E mock 第 1 题：请介绍你在后端项目中最有挑战的一次设计决策。",
      focus: "项目深度",
      expectedPoints: ["架构权衡", "量化结果"],
      followups: ["这个决策的代价是什么"],
    },
    {
      prompt: "E2E mock 第 2 题：你如何排查一条长尾延迟明显的接口？",
      focus: "系统排查",
      expectedPoints: ["分层定位", "数据支撑"],
      followups: ["如何防止复发"],
    },
    {
      prompt: "E2E mock 第 3 题：谈谈你对团队协作中代码评审的看法。",
      focus: "协作",
      expectedPoints: ["质量门禁", "知识共享"],
      followups: ["如何处理分歧"],
    },
  ],
};

/** 报告点评请求的固定 JSON（字段与 practice/report.rs 的 GeneratedReview 对齐）。 */
const REVIEW_JSON = {
  perQuestion: [
    {
      index: 1,
      score: 4,
      strengths: ["E2E mock：回答结构清晰"],
      issues: ["E2E mock：缺少量化结果"],
      modelAnswer: "E2E mock：用 STAR 结构补充量化效果",
    },
  ],
  dimensions: { contentDepth: 4, structureClarity: 3.5, fluency: 4, jobFit: 3.5 },
  totalScore: 3.8,
  topSuggestions: ["补充量化结果", "先结论后细节", "控制语速"],
};

/** TTS 返回的原始 PCM 字节（偶数长度，16bit 单声道静音 10ms@16kHz）。 */
const PCM_BYTES = Buffer.alloc(320);

const MOCK_ASR_TEXT = "E2E mock 语音转写";

const MODELS = ["e2e-mock-asr", "e2e-mock-llm", "e2e-mock-tts"];

function readBody(request) {
  return new Promise((resolveBody, rejectBody) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => resolveBody(Buffer.concat(chunks)));
    request.on("error", rejectBody);
  });
}

function json(res, status, payload) {
  const body = JSON.stringify(payload);
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(body),
  });
  res.end(body);
}

function lastUserContent(payload) {
  const messages = Array.isArray(payload?.messages) ? payload.messages : [];
  const users = messages.filter((message) => message?.role === "user");
  return users.at(-1)?.content ?? "";
}

/** 依据提示词里的 JSON 模板字样分发（见模块注释）。 */
function classifyChatRequest(userContent) {
  if (userContent.includes('"questions":')) return "plan";
  if (userContent.includes('"perQuestion":')) return "review";
  return "turn";
}

export function createMockOpenAiHandler() {
  /** @type {Array<{method: string, path: string, kind: string, userContent: string}>} */
  const requests = [];
  const handler = async (req, res) => {
    const path = new URL(req.url ?? "/", "http://localhost").pathname;
    const body = await readBody(req);
    let kind = "other";
    let userContent = "";
    if (req.method === "GET" && path.endsWith("/models")) {
      json(res, 200, { data: MODELS.map((id) => ({ id })) });
      requests.push({ method: req.method, path, kind: "models", userContent });
      return;
    }
    if (req.method === "GET" && path === "/__health") {
      json(res, 200, { ok: true });
      return;
    }
    if (req.method === "GET" && path === "/__requests") {
      json(res, 200, { requests });
      return;
    }
    if (req.method === "POST" && path.endsWith("/chat/completions")) {
      let payload = {};
      try {
        payload = JSON.parse(body.toString("utf8"));
      } catch {
        payload = {};
      }
      userContent = lastUserContent(payload);
      kind = classifyChatRequest(userContent);
      const content = kind === "plan"
        ? JSON.stringify(PLAN_JSON)
        : kind === "review"
          ? JSON.stringify(REVIEW_JSON)
          : MOCK_REPLY_TEXT;
      json(res, 200, {
        id: "chatcmpl-e2e-mock",
        object: "chat.completion",
        choices: [{ index: 0, message: { role: "assistant", content }, finish_reason: "stop" }],
      });
      requests.push({ method: req.method, path, kind, userContent });
      return;
    }
    if (req.method === "POST" && path.endsWith("/audio/speech")) {
      res.writeHead(200, {
        "Content-Type": "application/octet-stream",
        "Content-Length": PCM_BYTES.length,
      });
      res.end(PCM_BYTES);
      requests.push({ method: req.method, path, kind: "tts", userContent });
      return;
    }
    if (req.method === "POST" && path.endsWith("/audio/transcriptions")) {
      json(res, 200, { text: MOCK_ASR_TEXT });
      requests.push({ method: req.method, path, kind: "asr", userContent });
      return;
    }
    json(res, 404, { error: { message: `E2E mock 未实现 ${req.method} ${path}` } });
  };
  return { handler, requests };
}

export async function startMockOpenAiServer({ host = "127.0.0.1", port = 0 } = {}) {
  const { handler, requests } = createMockOpenAiHandler();
  const server = createServer((req, res) => {
    handler(req, res).catch(() => {
      try {
        json(res, 500, { error: { message: "E2E mock 内部错误" } });
      } catch {
        // 响应已发出的情况下忽略
      }
    });
  });
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(port, host, () => resolveListen());
  });
  const address = server.address();
  return {
    port: address.port,
    baseUrl: `http://${host}:${address.port}/v1`,
    requests,
    close: () => new Promise((resolveClose) => server.close(() => resolveClose())),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const requested = Number(process.argv[2] ?? 0);
  startMockOpenAiServer({ port: Number.isFinite(requested) ? requested : 0 }).then((server) => {
    console.log(`E2E mock OpenAI 服务器已启动：${server.baseUrl}（Ctrl+C 退出）`);
  });
}
