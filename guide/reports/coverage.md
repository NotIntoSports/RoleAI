# RoleAI 覆盖率报告（Rust 与前端）

> 生成：2026-09-30，由 lane-I（I05）在本机实际运行得出。
> 一键复现：
>
> ```powershell
> # 前端（vitest + @vitest/coverage-v8）
> npm run test:coverage:ui
> # Rust（cargo-llvm-cov，需要 rustup component add llvm-tools-preview）
> cargo llvm-cov --manifest-path src-tauri/Cargo.toml --summary-only
> ```
>
> 所有数字来自当次运行输出，未做任何修饰。CI 上每次 push 都会在 `coverage` job
> 重新生成同样的两张表并写入 job summary（HTML 报告见 artifact `coverage-reports`）。

## 环境

| 项 | 值 |
| --- | --- |
| CPU | Intel(R) Core(TM) Ultra 7 255H |
| 系统 | Microsoft Windows 11 家庭版（10.0.26200） |
| Rust | rustc 1.96.0，llvm-tools-preview，cargo-llvm-cov 0.9.1 |
| 前端 | vitest 5.0.0 + @vitest/coverage-v8 5.0.0 |
| 日期 | 2026-09-30 |

## 总览

| 范围 | 工具 | 总覆盖率（主要口径） |
| --- | --- | --- |
| Rust（`src-tauri/src/**`） | cargo-llvm-cov 0.9.1 | 区域 78.44% · 行 79.46%（704 单测 + 4 集成） |
| 前端（`src/**`，排除 css 与生成代码） | @vitest/coverage-v8 | 语句 75.5% · 分支 69.42% · 行 78.21%（48 文件 419 用例） |

## Rust 各模块（按区域覆盖率升序，最弱在前）

| 模块 | 区域覆盖率 | 行覆盖率 | 区域（总数/未覆盖） |
| --- | ---: | ---: | --- |
| commands | 44.21% | 46.27% | 6508 / 3631 |
| secrets | 70.09% | 72.06% | 224 / 67 |
| obs | 71.22% | 59.92% | 688 / 198 |
| audio | 77.54% | 78.56% | 3722 / 836 |
| migrate | 78.62% | 85.82% | 1726 / 369 |
| processes | 79.75% | 78.18% | 405 / 82 |
| services | 80.33% | 82.99% | 7055 / 1388 |
| providers | 81.78% | 80.48% | 3831 / 698 |
| prerequisites | 83.76% | 84.44% | 1139 / 185 |
| （根目录：lib.rs / startup.rs / contracts.rs 等） | 84.19% | 83.84% | 1695 / 268 |
| database | 88.24% | 98.58% | 289 / 34 |
| materials | 90.30% | 93.99% | 3896 / 378 |
| diagnostics | 90.98% | 93.14% | 366 / 33 |
| livestream | 92.82% | 92.81% | 418 / 30 |
| sessions | 93.93% | 96.15% | 2289 / 139 |
| runtime | 94.09% | 95.90% | 2115 / 125 |
| config | 94.31% | 94.99% | 1213 / 69 |
| practice | 97.11% | 98.01% | 2285 / 66 |

说明：`commands` 是 Tauri IPC 命令薄层，多数逻辑在 service 层被直接单测覆盖；
其低数字主要反映“命令包装函数本身”未被逐个经 IPC 通道调用（部分文件如
`commands/voice.rs`、`commands/obs.rs` 为设备/硬件通道，仅能在真机手测），
不等于业务逻辑裸奔。区域（region）是 llvm-cov 的分支级覆盖口径，比行覆盖更严格。

## 前端各目录（按语句覆盖率升序，最弱在前）

| 目录 | 语句 | 分支 | 行 |
| --- | ---: | ---: | ---: |
| src/api | 40.24% | 100% | 40.24% |
| src/demo/backend | 66.07% | 49.54% | 69.34% |
| src/features/services | 69.06% | 72.61% | 69.72% |
| src/screens/services | 79.09% | 65.02% | 80.66% |
| src/features/updater | 84.21% | 76.19% | 83.33% |
| src/features/session | 80.87% | 77.82% | 85.57% |
| src/features/migrate | 94.44% | 100% | 94.44% |
| src/features/roles | 93.1% | 96.87% | 92.59% |
| src/features/practice | 91.24% | 89.32% | 92.21% |
| src/demo | 92.85% | 90% | 92.59% |
| src/app | 94.11% | 86.36% | 97.61% |
| src/components | 100% | 100% | 100% |
| src/screens（materials/practice/records/workspace） | 100% | 100% | 100% |

说明：`src/api`（commands.ts 薄层）是 @tauri-apps IPC 包装，多数函数只有
类型转发，vitest 侧以 mockIPC 覆盖了 demo 主链路；`src/generated/**`（生成代码）
与纯样式 `*.css` 不计入。

## 已知局限

- Rust 分支列显示 `-`：llvm-cov 的区域口径已含分支语义，未单开 branch 维度。
- 覆盖率不作为门禁阈值（本仓库明确不“为数字写测试”），只作为补洞指引：
  I06 按“最弱且最关键”原则补测。
- 忽略项：cargo 测试 15 个 `#[ignore]`（需真实在线服务，不进 CI），与门禁口径一致。
