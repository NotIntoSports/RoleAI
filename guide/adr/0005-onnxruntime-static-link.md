# 0005 — onnxruntime 构建期静态链接

- 状态：已采纳
- 日期：2026-09-26（Silero VAD 落地时确立，Smart Turn 复用）

## 背景

VAD（Silero）与语义断句（Smart Turn）都靠 ONNX 模型推理。Rust 生态的事实标准是 `ort` crate，它默认可以走 `ort-load-dynamic`——运行时搜索并加载 `onnxruntime.dll`。

## 问题

Windows 的 DLL 搜索顺序会命中系统目录：用户机器上的 System32 若有一个别的软件装的旧版 onnxruntime.dll，应用就可能加载它——轻则算错，重则崩。这在真机上真的发生过。

## 选项

1. **运行时动态加载 + 随包带 DLL**——搜索顺序问题依旧存在。
2. **构建期静态链接 onnxruntime**（`ort` 关闭 `default-features`，即关闭 load-dynamic）。

## 决策

选项 2。静态链接 onnxruntime 1.22；`ort` 钉版 2.0.0-rc.10（rc.11+ 移除 CPUExecutionProvider，升级会编译失败，Cargo.lock 一并钉住）；VAD 与 Smart Turn 复用同一个钉版 ort，不引入第二个版本。原因完整记录在 [Cargo.toml](../../src-tauri/Cargo.toml) 注释里。

## 后果

- 好处：行为不随用户机器变化；两个模型（Silero opset 16、Smart Turn int8，约 8.7 MB）随包分发，推理全程离线。
- 代价：二进制增大数 MB、全量编译变慢；升级 onnxruntime 需要显式动 Cargo.lock。
