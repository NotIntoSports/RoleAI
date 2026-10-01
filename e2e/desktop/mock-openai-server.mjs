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
 *   POST {base}/chat/completions      → LLM：会话轮回复
 *   POST {base}/audio/transcriptions  → ASR：{ text }（文本输入 e2e 不经过，
 *                                        仅为覆盖卡片要求的最小子集）
 *   POST {base}/audio/speech          → TTS：原始 PCM 字节（response_format=pcm，
 *                                        parse_tts_pcm 要求偶数字节）
 *
 * chat/completions 一律按普通会话轮返回固定文本（原题单/点评分支随模拟面试模块移除）。
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
export const MOCK_REPLY_TEXT = "这是来自 E2E mock 的回复：语音会话链路工作正常。";

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

/** chat/completions 只服务普通会话轮（题单/点评分支随模拟面试模块移除）。 */
function classifyChatRequest() {
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
      const content = MOCK_REPLY_TEXT;
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
