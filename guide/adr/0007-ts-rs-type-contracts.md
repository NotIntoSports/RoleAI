# 0007 — ts-rs 生成前端类型契约

- 状态：已采纳
- 日期：2026-09 上旬

## 背景

Tauri 应用里，Rust 命令的参数与返回值经 IPC 序列化给 TypeScript 前端。手写两份类型定义必然漂移：后端改了字段名，前端在运行时才炸。

## 选项

1. **手写 TS 类型**——漂移是时间问题。
2. **specta / tauri-specta**——功能全，但绑定较深，与既有命令层结构耦合大。
3. **ts-rs**：在 DTO 上派生，`cargo test` 自动生成 `src/generated/bindings.ts`，前端 `tsc` 直接消费。

## 决策

选项 3。全部跨进程 DTO 集中在 `contracts.rs`（ts-rs 12，钉版），生成物提交在 `src/generated/bindings.ts`；前端只允许经 `src/api/commands.ts` 调用 IPC（`frontend-contract` 契约测试强制）。配套的安全面基线把生成的绑定文件也纳入哈希——契约文件的任何变化都必须显式可评审。

## 后果

- 好处：后端改字段 → `cargo test` 重新生成 → 前端 `tsc` 立即报错，漂移在编译期暴露；DTO 有单一事实来源。
- 代价：换行符陷阱——`cargo test` 以 LF 重新生成文件，Windows 工作区可能显示为"被修改"，需要按 `--ignore-cr-at-eol` 判断后还原（流程写进了贡献指南）；生成文件不允许手改。
