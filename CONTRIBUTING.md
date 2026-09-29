# Contributing

感谢关注 RoleAI。提交前请先阅读根目录的 [AGENTS.md](AGENTS.md)（项目开发规则）与 [README.md](README.md)。

**红线**：任何 PR 中不得出现 API Key、RTC Token、候选人数据、本机配置、日志原文或构建产物。安全漏洞走 [Security Advisory](https://github.com/NotIntoSports/RoleAI/security/advisories/new)，不要开公开 Issue。

## 开发环境

- Windows x64
- Rust 1.96（`rustup` 安装）
- Node.js 24
- Microsoft Edge WebView2 Runtime
- .NET 10 SDK（仅构建 AudioBridge 子进程需要；见 [native/AudioBridge/README.md](native/AudioBridge/README.md)）

```powershell
npm install
npm run tauri:dev      # 开发调试
```

## 分支与提交规范

- 从 `main` 拉特性分支，一个主题一个分支。
- 提交信息格式：`类型(范围): 中文描述`，类型取 `feat` / `fix` / `refactor`（纯搬移注明"（纯搬移）"）/ `test` / `docs` / `chore` / `ci` / `perf`。例如 `fix(realtime): 回答开始超时看门狗`。
- 一个提交只做一件事；不要 `git add -A`，用显式路径暂存。

## 测试与验证

```powershell
npm run test:tauri          # 全量门禁：cargo test + vitest + node 契约测试 + 前端构建
npm run test:tauri-package  # 打包冒烟（需要已构建的 exe；CI 不跑）
```

提交前至少跑一遍 `npm run test:tauri`；只改文档可以只跑相关的契约测试（如 `node --test tests/tauri/readme-ci-contract.test.mjs`）。改动依赖清单后跑 `npm audit --audit-level=high`。

几类容易被契约测试拦下的回归：

- **前端 IPC 契约**：生产代码只有 `src/api/commands.ts` 能 import `@tauri-apps/api/core`。
- **README 契约**：README 与 CI 只描述 Tauri 单一产品路径，必须包含标准命令，必须保留"托管 OBS……已延期"的表述。
- **安全面基线**：见下一节。

## 安全面基线（改到锁定文件时必读）

`tests/tauri/security-surface-baseline.json` 用 SHA-256 锁定了一批安全关键文件（能力白名单、`lib.rs`、`contracts.rs`、`src/api/commands.ts`、生成的类型绑定等）。改动其中任何一个文件：

1. 按该 JSON `comment` 字段的步骤重算哈希（UTF-8、LF 归一后取 SHA-256）；
2. 更新 JSON；
3. 与代码改动放在**同一个提交**里，评审时两件事一起看。

不许扩大 `src-tauri/capabilities/main.json` 的权限；新增 Tauri 命令只加对应的最小权限条目。

## 生成的类型绑定

`cargo test` 会用 ts-rs 重新生成 `src/generated/bindings.ts`。Windows 工作区可能因换行符（CRLF/LF）显示为"被修改"——先用 `git diff --ignore-cr-at-eol --stat -- src/generated/bindings.ts` 判断：无实质差异就 `git checkout --` 还原，不要提交；只有 DTO 真的变了才提交它。该文件不允许手改。

## 引入新依赖

遵循 [AGENTS.md](AGENTS.md) 的开源优先流程：

1. 先找现有能力、官方 SDK 或活跃维护的成熟依赖，不从零手写；
2. 评估许可证、维护活跃度、Windows 兼容性、体积与成本、安全与数据去向、接入与维护成本，并在工作记录中写清结论；
3. 同一类能力只引入一个库；版本钉死（Cargo 用 `=x.y.z`，npm 用精确版本）；
4. 如果决定自研（比如某个十行的小工具函数），说明为什么现有方案不可用。

近期案例见 [guide/adr/](guide/adr/README.md)（如 ADR-0008：为什么不引入重试库）。

## 文档

- 面向使用者的文档放 `guide/`（架构、语音管线、知识库、安全、配置、故障排查、ADR）；
- README、文档里的每条能力、每个数字都要能在代码、测试输出或基准报告里找到出处——**不编造**；
- 测试数量等统计数字用保守表述，并注明测量环境。
