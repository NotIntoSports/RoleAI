# 延迟可观测性指南

RoleAI 的核心体验是实时语音延迟。本文说明延迟数据从哪里来、在哪里看、如何导出分析。

## 每轮时间线有哪些阶段

每一轮对话按线路模式记录分阶段毫秒数（写入会话事件的 `turn_meta`）：

**实时线路（Realtime）**，相对泵启动（≈会话开始）：

| 字段 | 含义 |
| --- | --- |
| `speechStartedMs` | 服务端 VAD 听到用户开口 |
| `speechStoppedMs` | 用户说完（断句判定锚点） |
| `transcriptDoneMs` | 转写完成 |
| `responseCreatedMs` | 应答请求发出 |
| `firstAudioMs` | 首包音频到达 |
| `responseDoneMs` | 回答完成 |

**级联线路（Cascade）**，相对轮次起点（语音成句 finalize）：

| 字段 | 含义 |
| --- | --- |
| `asrDoneMs` | ASR 转写完成 |
| `retrievalDoneMs` | RAG 资料检索完成 |
| `llmFirstTokenMs` | LLM 首个 token |
| `llmDoneMs` | LLM 补全结束 |
| `ttsDoneMs` | TTS 合成完成（即级联的「首响」） |

另有 `playbackStartedMs` / `playbackDoneMs`（播放起止，预留字段）。缺失的阶段为空；
旧版本会话没有时间线时界面显示「无数据」。

## 在哪里看

1. **会话页**：每个 AI 回复下方有一条延迟瀑布条（分段显示各阶段占比与总耗时），
   点击展开分阶段明细。可在 设置 → 性能面板 关闭。
2. **设置 → 性能面板**：按线路 + 模式的首响 p50/p95、分阶段 p50/p95 表格、
   最近 50 轮总延迟折线（只统计最近 200 轮）。
3. **记录详情页**：每次会话的逐轮时间线随记录保存，可导出做离线分析。

## 导出

记录详情页提供两个导出入口（写入应用数据目录 `exports/` 子目录，路径显示在页面提示中）：

- **导出延迟数据**（`<会话ID>-latency.latency.csv`）：每轮一行，列依次为
  `turnIndex, createdAt, mode, interrupted, totalMs,` 全部时间线字段 `, userText, assistantText`。
  无时间线的旧轮次不写入。适合 Excel / pandas 直接分析。
- **导出时间线**（`<会话ID>-latency.latency.trace.json`）：Chrome Trace Event Format JSON，
  每轮一个进程轨道（process_name 标注轮次与模式），相邻锚点之间为一个 Complete 片段
  （微秒时间戳）。打开方式：
  1. 浏览器打开 [ui.perfetto.dev](https://ui.perfetto.dev)（本地渲染，不上传数据），
     或 Chrome 地址栏 `chrome://tracing`，点击 load 载入该 JSON；
  2. 每个进程轨道即一轮对话，片段长度即该阶段耗时，可直观看到慢在哪一段。

导出文件只包含延迟时间线与轮次文本，不包含提示词、凭据或音频数据。

## 相关实现

- 时间线结构与打点：`src-tauri/src/services/realtime_pump/shared.rs`（`TurnTimeline`）、
  `src-tauri/src/runtime/cascade.rs`（级联打点）。
- turn_meta 写入：`src-tauri/src/services/sessions/finalize.rs`。
- 汇总统计：`src-tauri/src/commands/system.rs`（`getDiagnosticsLatencySummary`）。
- 导出实现：`src-tauri/src/sessions/latency_export.rs`。
