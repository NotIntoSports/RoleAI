# 更新日志

本项目的所有显著变更都记录在本文件。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 计划中

- 在线演示版（浏览器里跑真实界面 + 模拟后端）
- 基准测试与离线音频评测报告（断句准确率、误打断率、端到端延迟）
- 托管 OBS、虚拟摄像头一键启停与快捷键 UI —— 已延期
- macOS / Linux 支持

## [0.1.0] - 2026-09-30

首个公开版本：本地优先的 Windows 实时语音 AI 角色助手。

### 新增

- **实时语音会话**：级联（ASR → LLM → TTS，三段可混搭任意 OpenAI 兼容服务）与端到端 Realtime（OpenAI / 阿里云 DashScope / 智谱三种协议方言）双模式；全双工对话，支持随时插话打断。
- **两级语义断句**：Silero VAD（32 ms 窗）+ Smart Turn 回合完成度模型（ONNX int8，随包分发，离线推理）。
- **回声治理**：浏览器 AEC 共用 AudioContext、播报期回声抑制窗与底噪追踪、当轮回声相关性判定、文本级回声闸门（归一化比对 + bigram Dice），杜绝自问自答循环。
- **断线恢复**：Realtime 断线指数退避重连（500 ms 起、30 s 封顶），上下文逐条确认回放并跳过 AI 历史轮次；回答开始超时看门狗。
- **本地知识库**：PDF / DOCX / 文本导入，语义分块，FTS5 + sqlite-vec 混合检索（倒数排序融合），备份与恢复。
- **会话记录**：每轮自动生成结构化纪要（摘要、优势、跟进、局限、证据），支持导出与两步确认删除。
- **角色系统**：面试官、HR、严苛面试官、求职者陪练、会议助手、直播讲解员、表达教练七个内置角色 + 自定义角色。
- **声音复刻**：智谱（上传→克隆两步）与阿里云 DashScope（含 qwen 单调用复刻）音色定制。
- **虚拟直播**：从本地产品资料生成分段讲稿，逐段确认讲解，图片 / 循环视频舞台。
- **会议助手**：C# AudioBridge 子进程对白名单内会议进程做本机回环采集；人工接管一键切回自己的麦克风。
- **安全**：API Key 存 Windows 凭据管理器（内存副本零化）；Tauri 能力白名单逐命令最小权限；安全面基线测试固定关键文件哈希；诊断导出限制在应用数据目录内。

### 变更

- 架构从 Electron + Next.js + Go Control API + Python Agent 多进程收敛为单一 Tauri 2 应用（Rust 核心 + WebView 界面 + 按需拉起的 C# 音频子进程）。
- 级联线路的连接失败与 HTTP 429/502/503/504 增加有界自动重试（最多 2 次，间隔 300 ms / 900 ms）；超时与鉴权失败不重试。
- 配置与数据沿用 `%APPDATA%\AI Virtual Assistant` 目录与 `AI_VIRTUAL_ASSISTANT_CONFIG` 环境变量，老用户升级可直接读取既有配置。

### 修复

- 修复 qwen Realtime 断连后全量回放引发的重连死循环（回放逐条确认并跳过 assistant 轮）。
- 修复 AI 播报声被麦克风回收导致的自问自答循环（文本回声闸门）。
- 修复回答长时间无开始事件时会话卡在等待态（超时看门狗恢复到可重试状态）。
- 修复播报期 AEC 压低人声概率导致的双讲无法打断（滑动窗计数 + 能量辅助判定）。
- 修复诊断导出可写到数据目录之外的问题（目的地路径校验，`DIAGNOSTICS_DESTINATION_INVALID`）。

### 安全

- ort 构建期静态链接 onnxruntime 1.22，避免运行时加载系统目录中的旧版 DLL。

[Unreleased]: https://github.com/NotIntoSports/RoleAI/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/NotIntoSports/RoleAI/releases/tag/v0.1.0
