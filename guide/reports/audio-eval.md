# 音频断句与打断离线评测报告

> 生成：2026-09-30，lane-C（C44）。全部数字来自本机实际运行，可一键复现。
> 环境：Intel Core Ultra 7 255H / Windows 11 26200 / rustc 1.96.0（与基准报告同机）。

## 方法

- 语料：Windows SAPI 合成的面试回答风格句子（中文为主、含 1 句英文），由 `scripts/eval-audio/build-fixtures.mjs` 拼装为 7 个确定性场景（全部时间间隔与混音参数来自固定种子 LCG）；合成语音比真人干净，这是方法层面的已知局限。
- 评测对象：**真实生产代码**——SileroVad、VadSegmenter、SmartTurnAnalyzer（仓库自带 ONNX）、BargeInMonitor，按 32ms 实时帧离线喂数，不连网。
- 运行器：`src-tauri/examples/eval_turns.rs`；复现命令：

```powershell
node scripts/eval-audio/build-fixtures.mjs
cargo run --manifest-path src-tauri/Cargo.toml --release --example eval_turns
# 结果：target/eval-results.json
```

- 断句延迟 = 检出语句的"喂数位置"与金标句尾之差（负值代表在真实句尾之前就落句 = 截断过早）；过早截断阈值 300ms。
- 打断场景的播报按 AEC 后 -12dB 背景建模；回声场景的麦克风输入只含用户语音 + 播报的 50–200ms 延迟、-10~-25dB 衰减泄漏（播报干信号不属于麦克风输入）。

## 断句结果

| 场景 | 档位 | 金标句 | 检出 | 过早截断率 | 延迟中位 | 延迟 p95 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| normal_qa（正常问答） | 只 VAD | 8 | 8 | 0% | 2340 ms | 4936 ms |
| normal_qa | VAD+Smart Turn | 8 | 8 | 0% | 2340 ms | 4936 ms |
| hesitation（句中犹豫） | 只 VAD | 4 | 4 | **50%** | 1.8 ms | 986 ms |
| hesitation | VAD+Smart Turn | 4 | 4 | **25%** | 986 ms | 1856 ms |
| noise SNR 20 dB | 只 VAD | 8 | 8 | 0% | 2340 ms | 4936 ms |
| noise SNR 10 dB | 只 VAD | 8 | 8 | 12.5% | 392 ms | 3513 ms |
| noise SNR 10 dB | VAD+Smart Turn | 8 | 8 | 0% | 408 ms | 3513 ms |
| noise SNR 5 dB | 只 VAD | 8 | 3 | 12.5% | 292 ms | 336 ms |
| noise SNR 5 dB | VAD+Smart Turn | 8 | 4 | 12.5% | 2308 ms | 2768 ms |
| echo（回声泄漏） | 只 VAD | 4 | 4 | 25% | 463 ms | 6195 ms |

## 打断结果

| 场景 | 判据 | 期望 | 检出/触发事件 | 首次触发延迟 |
| --- | --- | ---: | ---: | ---: |
| barge_in（真打断 ×4） | 生产 BargeInMonitor（含底噪门控） | 4 | 4/4 | 217 ms |
| barge_in（真打断 ×4） | 阈值对照（连续 2 窗 ≥0.5，闸门关） | 4 | 4/4 | — |
| echo（回声泄漏 ×4） | 生产 BargeInMonitor | 0 | **持续误触发（75 事件）** | — |
| echo（回声泄漏 ×4） | 阈值对照 | 0 | 6 事件 | — |

## 结论

1. **两级断句的价值有数据支撑**：句中犹豫场景的过早截断率从 50%（只 VAD）降到 25%（VAD+Smart Turn），且被截断句的延迟从"立即截断"（1.8ms）改善到约 1s。Smart Turn 挽回的都是迟疑填充词后的半句。
2. **正常问答的句尾提交延迟（中位 2340ms）远大于静音判定的设计值（~700ms）**：延迟由分段器"保持态预算"（约 2.5s 强制提交）主导，说明连续语音流里句尾判定很少走"700ms 静音"路径。这是本次评测暴露的主要问题，已记录在 lane 账本（"发现"），调参属后续卡，不在本卡进行。
3. **噪声鲁棒性**：SNR 20dB 与安静环境几乎一致；SNR 10dB 出现零星截断；SNR 5dB 下 VAD 漏检严重（8 句只出 3 句），Smart Turn 召回其中 1 句。
4. **打断检出**：真实打断 4/4 检出，首次触发延迟中位 **217ms**——对"打断播报"的体感足够快。
5. **回声场景是当前短板**：无 AEC 的合成回声（-10~-25dB 泄漏）下，生产 monitor 与阈值对照都会持续误触发——**生产链路对回声的抑制依赖上游 AEC**（WebView/系统处理），本离线评测未建模 AEC，数字代表"AEC 失效时"的下界行为。

## 局限性（诚实清单）

- SAPI 合成语音比真人干净、韵律单一：真实环境的断句与误打断数字会更差。
- 回声场景未建模 AEC 与扬声器-麦克风频响差异，误触发数字**不代表真实使用表现**，只说明"没有 AEC 兜底时的风险敞口"。
- 打断判据的"闸门关"对照是评测器内实现的纯阈值规则（非生产代码），仅用于量化生产 monitor 底噪门控的作用边界。
- 句尾延迟对"保持态预算"参数敏感；调参实验（含前后对比）另行开卡，本报告不含调参后数字。
