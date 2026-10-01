# RoleAI 变异测试报告（核心纯逻辑模块）

> ⚠️ **状态（2026-10-01）：部分完成。** 长跑因用户决定提前收尾而中断（lane-I I07）。
> **仅 `audio/barge_in.rs` 完成了完整的变异测试与补测闭环**；`audio/segmenter.rs` 补测已写入
> （模块测试全绿）但其定向复验未运行；`materials/chunk.rs` 仅写入补测、变异轮未完成；
> `practice/metrics.rs` 与 `services/echo_guard.rs` **完全未运行**（practice 模块后已随模拟面试
> 训练模块整体移除，2026-10-01 用户决定，其变异测试不再需要）。下文保留的数据均来自
> 中断前已完成的真实运行，其余为空。

## 环境

| 项 | 值 |
| --- | --- |
| CPU | Intel(R) Core(TM) Ultra 7 255H |
| 系统 | Microsoft Windows 11 家庭版（10.0.26200） |
| Rust | rustc 1.96.0 |
| 工具 | cargo-mutants 27.1.0（sourcefrog，MIT）+ sccache 0.18.0（Apache-2.0，deps 跨副本缓存），均 `cargo install --locked` 装为工具 |
| 日期 | 2026-09-30 ~ 2026-10-01 |

## 复现入口

```powershell
powershell -ExecutionPolicy Bypass -File scripts\run-mutants.ps1
# 单模块：-Modules barge_in ；定向复验：-OnlyRegex "barge_in\.rs:(47|92):" ；
# 分片防中断：-Shard 1/2 ；逐模块流水：-NoShuffle
```

## 范围与方法

计划对手册指定的 5 个纯逻辑模块跑（无 IO/设备/真实时间依赖，变异结果可解释）：

`services/echo_guard.rs`、`audio/segmenter.rs`、`audio/barge_in.rs`、`materials/chunk.rs`、
`practice/metrics.rs`（最后一个已随模拟面试训练模块移除，见下）。

每轮先跑未变异基线（必须全绿），然后对每个变异体执行：注入 → 编译 → 跑全量 lib 测试；
测试失败/编译失败/超时记为「杀死」，测试全过记为「存活」。存活变异体逐个人工归因：
**测试缺失 → 补测试**，**等价变异/防御代码 → 记录不追**。

## 状态总览（诚实版）

| 模块 | 变异轮状态 | 补测状态 | 结论 |
| --- | --- | --- | --- |
| audio/barge_in.rs | **已完成**（69 变异体全测） | 已补 4 个测试，模块测试全绿 | 分数 95.7%（66 杀 / 2 存活记账 / 3 unviable） |
| audio/segmenter.rs | 部分完成：57 变异体测完一轮（36 严格杀 + 8 超时杀 + 4 unviable + 9 存活），**补测后的定向复验未运行** | 已补 2 个测试，模块测试全绿 | 复验前分数 77.2%；补测效果未复验，**不可引用为最终分数** |
| materials/chunk.rs | **未完成**（仅早期分片数据，未形成完整一轮） | 已补 2 个测试，模块测试全绿（74 passed） | 无变异分数 |
| practice/metrics.rs | 模块已整体移除（2026-10-01 用户决定），变异测试不再需要 | 模块已删除 | — |
| services/echo_guard.rs | **未运行** | 无 | — |

## audio/barge_in.rs（唯一完成闭环的模块）

69 变异体：补测前 33 严格杀死、9 超时杀死、3 unviable、16 存活、8 因中断未测；
补测后（新增 4 个断言/测试，其中 8 个未测变异体也在大轮后小轮补齐）：**66 杀死 / 2 存活记账 /
3 unviable，分数 66/69 = 95.7%**（若把 unviable 从分母剔除则为 100%）。模块测试 9 → 13，全绿。

补测清单：

| 新增测试 | 钉住的行为 | 杀死的存活变异体 |
| --- | --- | --- |
| `debug_pins_voiced_run_buffer_and_floor_after_two_windows` | Debug 暴露的 buffered_windows/voiced_run/echo_floor 数值 | fmt `/→%`、`/→*`；ingest `/→*`；`+→*`（voiced_run 卡 0） |
| `echo_floor_primes_to_min_then_rises_slowly_and_drops_fast` | 底噪三段式：建底期取 min（16 窗递增幅钉首窗）、稳态仅上浮 2%、安静窗立刻下探 | `track_echo_floor` 的 `<→==`、`<→>`、`<→<=`、`<=→>`（建底初始化）、`+=→*=`（两处）、`||→&&`、`<→>`；ingest `/→%`、`/→*` |
| `late_trigger_stages_exactly_the_capped_40_window_history` | 触发快照 = 封顶 40 窗 + 触发当窗（41 窗整） | `122:51 *→/`（缓冲清空）；连带钉住 120 窗口上限 |
| `assisted_voicing_requires_a_built_echo_floor` | 静音（底噪恒 0）时概率 [0.3,0.4) 的窗口不得借道能量辅助触发，且不得进入触发态 | `96:33 >→>=`（底噪 0 时刻的边界）；补 `!triggered()` 断言防 `triggered→true` |

存活记账（2，均不补测）：

- `102:28 replace >= with <`：`voiced_prob >= 0.3` 只是 debug! 打点的门。断言它需要订阅 tracing
  进程级 callsite，与 segmenter 的 WARN 捕获用例天然互扰（I06(4/6) 已记录同一约束），记账不追。
- `155:23 replace < with <=`（`window_rms < self.echo_floor`）：仅在 rms == floor 时与原语义分叉，
  而两分支在该时刻都保持底噪不变（赋值同值 / 上浮量为 0），属等价变异，记账不追。

另有 3 个 unviable（编译器/类型系统拒绝注入）不计入分数分母的存活集合。

## audio/segmenter.rs（补测已写入，复验未运行）

中断前一轮：57 变异体中 36 严格杀死、8 超时杀死（`take` 系列变异让清队列循环真死循环、
预卷 `>→<` 让 pop 循环真死循环——均为变异导致的真挂起，非负载假象）、4 unviable、9 存活。
复验前分数 77.2%。

存活集中暴露的盲区：`reset()` 与 trait 对象侧 `ready/dropped/reset` 转发零断言；
常规话轮（非 25s 封顶路径）长度从未被逐帧钉住。

已写入的补测（模块测试 12 → 14，全绿；**但未做变异轮定向复验，下列"预期杀死"未经证实**）：

| 新增测试 | 钉住的行为 | 预期杀死的存活变异体 |
| --- | --- | --- |
| `utterance_length_is_exact_preroll_plus_speech` | 常规话轮逐帧精确：预卷 10 + 激活后 9 有声 + 35 静音 = 54 帧 | `62:12 delete !`（状态机反转）、`64:39 >→>=`（预卷少 1 帧）、`68:32 >=→<`（激活时序提前 2 帧） |
| `reset_returns_to_fresh_state_and_trait_forwarding_is_honest` | 溢出计数经 dyn 转发为 1；复位后队列/丢弃计数清零；复位后连续两轮话轮与全新实例逐字节一致 | `55:9 reset→()`、`137:9 trait reset→()`、`131:9 trait ready→true`、`134:9 trait dropped→0/1`、`102:9 clear_current→()`（第二轮脏状态） |

**后续如恢复此线：先 `-OnlyRegex` 定向复验上表 8 个变异体，再补测其余模块。**

## materials/chunk.rs / services/echo_guard.rs

chunk.rs：变异轮被中断，仅有早期分片数据，不构成可引用的分数；已写入 2 个补测
（模块测试全绿）。echo_guard.rs：未运行，无任何数据。

## 已知限制

- 超时计为杀死：cargo-mutants 把超时的变异体记为 caught。满载下个别慢测试可能把"负载变慢"
  误判成超时杀死。已完成部分的做法：RTT=4 稳定基线 + 240s 超时下限；barge_in 的 9 个超时变异体
  全部用 jobs=1 低负载单独复验（6 个转为真实测试失败击杀、2 个确认为真死循环）；segmenter 的 8 个
  超时经分析均为变异导致的真死循环（`take` 清队列、预卷 pop 循环），非负载假象。
- 每个变异体跑全量 lib 测试（740+ 用例）：单个变异体成本 ~3.5 分钟（4 路并行摊薄后 ~1.5 分钟/个），
  全量一轮数小时，因此只覆盖 5 个纯逻辑模块、不进 CI；运行期间被外部中断过，
  通过分片（`--shard`）与 outcomes 增量落盘恢复，但最终仍因提前收尾未跑完全部模块。
- `2 * intersection / total` 一类浮点变异若被杀，依赖的是阈值附近的判别用例
  （echo_guard 0.84 vs 0.85 的实测注释），不是浮点精度断言。
