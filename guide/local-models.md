# 本地模型指南（Ollama / LM Studio / 本地 Whisper / 本地 TTS）

RoleAI 支持把语音链路的每一段（语音识别 ASR、大模型 LLM、语音合成 TTS、向量嵌入）接到**本机运行**的开源服务上。全部接入走项目既有的 **OpenAI 兼容协议**，不需要额外插件：本地服务启动后，在 RoleAI 里像选择云端供应商一样选择它们即可。

> 适用场景：数据不出本机（面试录音、简历、产品资料不发给第三方）、无网环境、控制成本。
> 代价：需要自己管理模型下载与硬件资源；没有独立显卡时 ASR/TTS 会明显变慢（详见每节说明）。

## 应用内预设

RoleAI 首次启动（或启动时检测到模板缺失）会自动播种以下**本机预设**，全部指向环回地址、不需要 API Key：

| 预设名称 | 接入地址 | 用途 |
| --- | --- | --- |
| Ollama（本机） | `http://127.0.0.1:11434/v1` | LLM 对话 / 嵌入向量 |
| LM Studio（本机） | `http://127.0.0.1:1234/v1` | LLM 对话 |
| 本地 Whisper（speaches） | `http://127.0.0.1:8000/v1` | ASR 语音识别 |
| 本地 TTS（Kokoro-FastAPI） | `http://127.0.0.1:8880/v1` | TTS 语音合成 |

使用方式：

1. 先启动对应的本地服务（下文各节）。
2. 打开 RoleAI「服务」页，选中对应预设供应商，点击「测试」——模型列表发现会列出该服务已加载/已安装的模型。
3. 在「语音线路」里把线路各段指向这些供应商，按你的实际模型名填写模型 ID（预设线路模板里的 ID 是常见默认值，需要按你安装的模型调整）。
4. 测试线路通过后再激活。模板线路不会自动激活，不影响你现有的云端线路。

已删除的预设会在下次启动时回补（与内置角色模板同一策略）；不使用它们不影响任何功能。

## Ollama（LLM / 嵌入）

- 官网与安装：<https://ollama.com/download>（Windows / macOS / Linux 安装包）。
- OpenAI 兼容层文档：<https://github.com/ollama/ollama/blob/main/docs/openai.md>。
- 启动：安装后 Ollama 常驻后台（Windows 任务栏图标）；如未运行，运行 `ollama serve`。
- 拉取模型（示例）：

  ```bash
  ollama pull qwen2.5:7b        # 对话模型，约 4.7 GB
  ollama pull nomic-embed-text  # 嵌入模型，768 维，约 274 MB
  ```

- 在 RoleAI 里：LLM 选「Ollama（本机）」+ 模型 `qwen2.5:7b`；知识库嵌入选预设模板 `nomic-embed-text`（维度 768，与模型卡一致；换其它嵌入模型时记得同步修改维度）。
- 体验说明：7B 级模型在现代 6 核以上 CPU 上约每秒几个到十几个 token，可用但等待感明显；有 8 GB 以上显存的显卡会流畅得多。模型体积与显存占用详见 Ollama 官方模型库。

## LM Studio（LLM）

- 官网与安装：<https://lmstudio.ai>（Windows / macOS / Linux）。
- 本地服务器文档：<https://lmstudio.ai/docs/app/api>。
- 启动：打开 LM Studio → 开发者（Developer）页 → 启动本地服务器（默认端口 1234，与预设一致）。
- 在 RoleAI 里：LLM 选「LM Studio（本机）」，模型 ID 用 LM Studio 里已加载的模型标识（可在其服务器页看到）。`测试`按钮会列出 `/v1/models` 返回的模型。
- 体验说明：与 Ollama 同量级——推理速度取决于模型大小与硬件；LM Studio 的优势是图形化挑选 GGUF 模型。

## 本地 Whisper（speaches，ASR）

- 项目与安装：<https://github.com/speaches-ai/speaches>（OpenAI API 兼容的 STT/TTS 服务器，底层 faster-whisper）。
- 端口与配置：<https://speaches.ai/configuration/>（默认 `UVICORN_PORT=8000`）。
- 启动（Docker，官方推荐）：

  ```bash
  docker run --rm -p 8000:8000 ghcr.io/speaches-ai/speaches:latest
  ```

- 在 RoleAI 里：ASR 选「本地 Whisper（speaches）」，模型 ID 形如 `Systran/faster-whisper-small`（首次请求自动下载；预置线路模板用的就是它）。
- 体验说明：`small` 级在纯 CPU 上实时率尚可，`large-v3` 基本需要显卡；断句准确率随模型增大而提升。RoleAI 侧的前端断句（VAD + 语义回合判断）在本地仍照常工作。

## 本地 TTS（Kokoro-FastAPI）

- 项目与安装：<https://github.com/remsky/Kokoro-FastAPI>（Kokoro-82M 的 OpenAI 兼容封装）。
- 启动（Docker，官方 README 的 CPU 版）：

  ```bash
  docker run --rm -p 8880:8880 ghcr.io/remsky/kokoro-fastapi-cpu:latest
  ```

- 在 RoleAI 里：TTS 选「本地 TTS（Kokoro-FastAPI）」，模型 `kokoro`，声音 ID 例如 `af_heart`（完整声音列表见该项目 README / `/v1/audio/voices`）。
- 已知限制（2026-09 核对官方文档）：Kokoro-FastAPI 公开的是 `/v1/audio/speech` 与 `/v1/audio/voices` 等端点，官方文档未列出 `GET /v1/models`，因此 RoleAI 的「测试」（模型列表发现）对该服务可能失败——这不影响实际合成，模型 ID 手填 `kokoro` 即可。
- 体验说明：Kokoro-82M 是轻量模型，纯 CPU 可实时合成短句；长段落会有可感知的延迟。

## 数据去向

- 本机预设全部指向 `127.0.0.1`，语音与文本不经过任何第三方服务器；对应供应商不配置 API Key 时，RoleAI 也不存在可上送的凭据。
- RoleAI 自身的遥测与密钥策略见[安全设计与数据去向](security.md)。

## 故障排查

- 「测试」失败/超时：先确认服务真的在监听（浏览器打开 `http://127.0.0.1:<端口>/v1/models` 应返回 JSON）。
- 端口冲突：各服务的端口都可配置（Ollama `OLLAMA_HOST`、LM Studio 服务器设置、speaches `UVICORN_PORT`、Kokoro-FastAPI 端口映射）；改完后在 RoleAI 供应商设置里同步修改接入地址。
- 模型拉取慢或失败：模型文件较大（GB 级），详见各服务官方文档的镜像/代理说明。
- 线路测试报供应商不可达：逐段检查 ASR/LLM/TTS 三个供应商的地址与模型 ID 拼写。
