# RoleAI 基准测试报告（Criterion）

> 生成：2026-09-30，由 lane-C（C34）在本机实际运行 `cargo bench` 得出。
> 一键复现：`powershell -File scripts/run-benchmarks.ps1`（等价于在 `src-tauri/` 下执行 `cargo bench`）。
> 所有数字来自当次运行的 criterion 输出，未做任何修饰；同一台机器两次全量运行的中位数差异通常在 ±8% 以内（见文末"波动与局限"）。

## 环境

| 项 | 值 |
| --- | --- |
| CPU | Intel(R) Core(TM) Ultra 7 255H |
| 内存 | 31.4 GB（物理总容量） |
| 系统 | Microsoft Windows 11 家庭版（10.0.26200） |
| Rust | rustc 1.96.0 (ac68faa20 2026-05-25) |
| 框架 | criterion 0.8.2（harness = false，sample_size 20–30，warm-up 500ms，测量 1s） |
| 构建 | `cargo bench` 默认 bench profile（开启优化） |
| 日期 | 2026-09-30 |

## 方法

- 输入全部为**确定性生成**信号（正弦 / LCG 白噪声 / 静音），不依赖外部音频文件；Silero VAD 与 Smart Turn 的 ONNX 模型走仓库自带资源（与单元测试同一份文件）。
- 混合检索的向量使用**确定性 LCG 哈希替身**（不调用任何 Embedding 服务），只衡量检索路径本身的 SQL/FUSE 开销，不衡量网络嵌入质量。
- 表格给出 criterion 的**中位数**与 **95% 置信区间**；吞吐为 criterion 折算值。明细（含概率密度图与历史对比）在 `src-tauri/target/criterion/`。

## 音频管线热路径

实时因子 = 基准覆盖的音频时长 ÷ 处理耗时，越大越好（>1 即跟得上实时）。

| 基准 | 含义 | 中位数 | 95% 置信区间 | 实时因子 |
| --- | --- | ---: | --- | ---: |
| pcm/resample_48k_to_16k_1s | 1 秒 48kHz 16-bit 单声道重采样到 16kHz（ASR 入口常规转换） | 100.8 µs | 99.0–103.0 µs | ~9 900× |
| pcm/ring_push_1536_samples_steady_state | 3 秒环形缓冲灌满后的稳态写入（32ms 块） | 64.7 ns | 63.7–65.6 ns | ~495 000× |
| vad/silero_single_frame_silence | Silero VAD 单窗（512 样本 = 32ms）推理，静音窗 | 79.7 µs | 78.3–81.4 µs | ~401× |
| vad/silero_single_frame_speech | 同上，正弦语音窗 | 81.9 µs | 79.5–84.7 µs | ~391× |
| smart_turn/score_4s_turn | Smart Turn 完整性判定一次：4 秒语音段（含 Whisper 对数梅尔特征 + ONNX 推理） | 68.2 ms | 61.1–76.4 ms | ~59× |
| segmenter/utterance_energy_ingest_32ms | 能量分段器稳态摄入 32ms 块 | 1.83 µs | 1.81–1.85 µs | ~17 600× |
| segmenter/vad_ingest_32ms_with_silero | VAD 分段器摄入 32ms 块（含一次 Silero 推理） | 83.2 µs | 81.9–84.7 µs | ~384× |
| segmenter/barge_in_ingest_32ms_with_silero | 打断检测摄入 32ms 块（重采样 + Silero + 底噪追踪） | 86.8 µs | 83.4–91.0 µs | ~369× |
| echo_guard/is_echo_typical_turn | 回声文本过滤一次判定（两个近期播报 vs 一条用户发言） | 3.48 µs | 3.42–3.55 µs | — |
| echo_guard/normalize_200_chars | 200 字符文本规一化 | 4.26 µs | 4.18–4.34 µs | — |
| echo_guard/gate_deadline_24k_bytes | 回声闸门排水截止时刻计算（24KB 播放数据折算） | 27.6 ns | 27.1–28.2 ns | — |

**解读**

- 全链路最贵的单帧环节是 Silero 推理（~80µs/32ms），整条"摄入→VAD→分段/打断"路径的实时因子都在 **~370× 以上**——音频热路径不是本应用的瓶颈，麦克风 32ms 块到达间隔内绰绰有余。
- Smart Turn 是唯一的"每轮一次"（而非每帧）环节：4 秒回答 68ms，占轮次总延迟（网络 + TTS 通常以秒计）不到 1%，且只在静音判定边界触发。
- 回声闸门的文本过滤与排水计算均为微秒/纳秒级，可忽略。

## 资料库（分块 / 混合检索 / 写入）

| 基准 | 含义 | 中位数 | 95% 置信区间 | 吞吐 |
| --- | --- | ---: | --- | --- |
| chunk/chunk_text/2000 runes | 2 千字资料语义分块 | 66.4 µs | 65.8–67.2 µs | 79.1 MiB/s |
| chunk/chunk_text/20000 runes | 2 万字资料分块 | 1.29 ms | 1.27–1.32 ms | 40.6 MiB/s |
| chunk/chunk_text/200000 runes | 20 万字资料分块 | 83.5 ms | 82.7–84.4 ms | 6.29 MiB/s |
| hybrid/index_chunks/484 chunks | 对 484 块做向量索引（本地哈希向量替身 + SQLite vec0 写入） | 18.4 ms | 17.8–18.9 ms | ~26 千块/s |
| hybrid/index_chunks/4904 chunks | 同上，4904 块 | 209.7 ms | 201.5–220.4 ms | ~23 千块/s |
| hybrid/search_hybrid_vector/484 chunks | 混合检索（关键词 + 64 维向量 RRF 融合），top 5 | 256.7 µs | 245.7–266.8 µs | ~1.89 M 块/s |
| hybrid/search_hybrid_vector/4904 chunks | 同上，4904 块 | 477.0 µs | 468.4–487.1 µs | ~10.3 M 块/s |
| hybrid/search_keyword/484 chunks | 纯关键词（FTS5），top 5 | 51.3 µs | 50.1–53.2 µs | ~9.4 M 块/s |
| hybrid/search_keyword/4904 chunks | 纯关键词，top 5 | 52.7 µs | 51.8–53.5 µs | ~93 M 块/s |
| sqlite/insert_text_ready_10k_runes | 单篇 1 万字资料落库（材料行 + 全文 + 分块 + FTS，单事务） | 3.99 ms | 3.68–4.27 ms | ~6.9 MiB/s |

**解读**

- 4900 块规模的混合检索中位数 **0.48ms**——对"翻资料回答问题"的场景（整轮 1–5 秒）完全无感；规模到 1 万块预计仍在毫秒级（检索路径为 SQL LIMIT + RRF，未观察到随规模超线性增长）。
- 关键词检索耗时与规模几乎无关（FTS5 倒排索引），混合检索的增量主要来自 sqlite-vec 向量扫描。
- 单篇万字资料入库 4ms，配合导入接口 8MB 上限，最坏情况导入也在秒级内完成。

## 复现

```powershell
git clone <repo>   # 本报告对应的分支 longrun/backend
cd RoleAI
powershell -File scripts/run-benchmarks.ps1
# 或等价：cd src-tauri; cargo bench
```

- 两个基准目标：`benches/audio_hotpath.rs`（音频）与 `benches/materials.rs`（资料库）。
- 环境差异（CPU 频率、后台负载、电源策略）会整体平移绝对值；同一环境内的**相对**结论（如"Silero 单帧 ≈80µs ≪ 32ms 帧长"）在数量级上稳定。

## 波动与局限

- criterion 每次 20–30 个样本、1 秒测量；两次全量运行的中位数差异普遍 ±2–8%，个别（smart_turn）可达 ±20%——表格结论请按数量级理解，不做百分位级比较。
- Smart Turn / Silero 的绝对耗时受 ONNX Runtime 线程调度影响明显，笔记本插电/电池策略下数字可能显著不同。
- 混合检索的向量是哈希替身：**只代表检索机制的开销，不代表真实 Embedding 向量的召回质量**（质量数字见 `guide/reports/rag-eval.md`，离线评测）。
- 200k 字符分块触发单篇 500 块上限截断，吞吐数字反映"完整分块 + 截断"的综合行为。
