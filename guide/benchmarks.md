# 基准测试与评测报告

RoleAI 的性能与质量数字不靠嘴说：仓库里带三个**可一键复现**的离线报告，全部由真实生产代码在本机跑出，语料与输入均为确定性生成（不依赖外部音频文件、不调用任何付费服务）。

## 报告一览

| 报告 | 回答的问题 | 复现命令 |
| --- | --- | --- |
| [Criterion 基准报告](reports/benchmarks.md) | 音频热路径、Smart Turn、混合检索各要多久？跟得上实时吗？ | `powershell -File scripts/run-benchmarks.ps1` |
| [音频断句与打断离线评测](reports/audio-eval.md) | 两级断句（VAD + Smart Turn）值多少？打断多快？回声场景的风险敞口多大？ | `node scripts/eval-audio/build-fixtures.mjs` 后 `cargo run --release --example eval_turns` |
| [RAG 检索离线评测](reports/rag-eval.md) | 混合检索机制（FTS5 + sqlite-vec + RRF）工作正常吗？ | `cargo run --release --example eval_rag` |

## 测试环境

三份报告均产生于同一台机器（2026-09-30）：

| 项 | 值 |
| --- | --- |
| CPU | Intel(R) Core(TM) Ultra 7 255H |
| 内存 | 31.4 GB |
| 系统 | Windows 11 家庭版（10.0.26200） |
| Rust | rustc 1.96.0 |

换机器绝对值会平移（电源策略、后台负载影响明显），但同环境内的相对结论（如「Silero 单帧推理远小于 32ms 块间隔」）在数量级上稳定。

## 关键数字速览

以下数字全部出自上表三份报告的当次运行，未做修饰：

- **音频热路径**：Silero VAD 单窗（32ms）推理中位约 80µs，整条「摄入 → VAD → 分段/打断」路径实时因子在 **370× 以上**——热路径不是瓶颈（[benchmarks.md](reports/benchmarks.md)）。
- **Smart Turn**：4 秒语音段完整性判定中位 68.2ms，每轮只触发一次，占轮次总延迟不到 1%（同上）。
- **打断（barge-in）**：合成语料真打断 4/4 检出，首次触发延迟中位 **217ms**（[audio-eval.md](reports/audio-eval.md)）。
- **两级断句价值**：句中犹豫场景的过早截断率从 50%（只 VAD）降到 25%（VAD + Smart Turn）（同上）。
- **混合检索**：4904 块规模检索中位 0.48ms（RRF 融合），关键词路径 52.7µs（[benchmarks.md](reports/benchmarks.md)）。
- **RAG 机制验证**：50 题离线题集上向量替身 R@5 84% / MRR 0.69——替身只评检索机制，不代表真实 Embedding 的语义上限（[rag-eval.md](reports/rag-eval.md)）。

## 已知局限（诚实清单）

- 音频评测语料由 Windows SAPI 合成，比真人干净；回声场景未建模 AEC，误触发数字代表「AEC 失效时」的下界行为。
- RAG 评测的向量是字符 bigram 哈希替身，84% 的 R@5 全部来自字面重叠；真实 Embedding 的召回质量需配置在线服务后另测。
- 句尾提交延迟中位 2340ms 由分段器「保持态预算」主导，高于 700ms 静音判定设计值——这是评测暴露的待调参项，报告原文有分析。

每份报告末尾都有完整的局限说明与复现步骤，欢迎在自己机器上重跑核对。
