# RoleAI 技术文章

深度长文，讲实现背后的取舍与踩坑。数字均出自仓库的[基准与评测报告](../benchmarks.md)，可复现。

- [用 Rust 做一个全双工实时语音助手：断句、回声与打断](2026-09-30-full-duplex-voice.md) —— turn-taking 的三个"什么时候"：两级断句、四道回声防线、滑动窗打断，以及 qwen 断连死循环的修复。
- [在桌面应用里做本地 RAG：SQLite + sqlite-vec 混合检索实践](2026-09-30-local-rag-sqlite-vec.md) —— 不装向量数据库的桌面 RAG：分块、RRF 融合，与一次把检索黑盒打穿的离线评测。
- [从五个进程到一个 exe：把 Electron + Go + Python 重构成 Tauri 的经验](2026-09-30-five-processes-to-one-exe.md) —— 多进程 AI 应用收敛为单一 Tauri 应用的动机、迁移方法与验收数字。
