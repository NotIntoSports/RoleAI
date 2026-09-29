<!-- 提交前请确认：PR 中不含密钥、候选人数据、本机配置、日志或构建产物。 -->

## 这个 PR 做了什么

<!-- 一两句话说明目的；修 bug 请附错误码/测试名。 -->

## 相关 Issue

<!-- 关联 Issue 请写 "Closes #123"；没有则写"无"。 -->

## 自查清单

- [ ] `npm run test:tauri` 全量通过（cargo test、vitest、node 契约、前端构建四段）
- [ ] 涉及打包/安装的改动跑过 `npm run test:tauri-package`
- [ ] 改动了安全面基线锁定的文件时，已按 `tests/tauri/security-surface-baseline.json` 注释重算哈希，并与代码改动在同一提交
- [ ] `src/generated/bindings.ts` 只在 DTO 真实变更时提交（`git diff --ignore-cr-at-eol` 排除换行符差异）
- [ ] 引入新依赖时已按 `AGENTS.md` 开源优先流程评估（许可证、维护、Windows 兼容、体积、安全），并钉死版本
- [ ] 文档/README 中的能力与数字都有代码或测试出处，无编造
