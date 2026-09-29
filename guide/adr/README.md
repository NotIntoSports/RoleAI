# 架构决策记录（ADR）

用 MADR 精简格式记录 RoleAI 的关键技术决策：背景、选项、决策、后果。读代码时觉得"为什么不用 X"的问题，多半在这里。

| 编号 | 决策 | 状态 |
| --- | --- | --- |
| [0001](0001-single-tauri-app.md) | 从 Electron + Next.js + Go + Python 多进程收敛为单一 Tauri 应用 | 已采纳 |
| [0002](0002-local-first-no-accounts.md) | 本地优先 + 凭据管理器，不做账号系统 | 已采纳 |
| [0003](0003-sqlite-sqlite-vec.md) | SQLite + sqlite-vec，不引入独立向量数据库 | 已采纳 |
| [0004](0004-cascade-and-realtime-dual-mode.md) | 级联与端到端 Realtime 双模式并存 | 已采纳 |
| [0005](0005-onnxruntime-static-link.md) | onnxruntime 构建期静态链接 | 已采纳 |
| [0006](0006-csharp-audiobridge.md) | 用 C# AudioBridge 子进程采集会议音频 | 已采纳 |
| [0007](0007-ts-rs-type-contracts.md) | ts-rs 生成前端类型契约 | 已采纳 |
| [0008](0008-hand-written-retry.md) | 自写有界重试，不引入重试库 | 已采纳 |
