# 安全设计与数据去向

RoleAI 处理两类敏感数据：**你的 API Key** 和 **你的对话内容**（录音、转写、资料库）。本文讲清楚这两类数据分别放在哪、谁能碰到、会发到哪里去。漏洞报告流程见根目录 [SECURITY.md](../SECURITY.md)。

## 威胁模型

先说清楚防什么、不防什么：

| 谁能碰到什么 | RoleAI 的态度 |
| --- | --- |
| 用户本人 | 数据的主人，全量可见：会话记录、资料库、配置都在本机 |
| 拿到机器的物理攻击者 | 同上——本机数据不做静态加密，磁盘加密请依赖 BitLocker 等系统机制 |
| 同一 Windows 用户下的其他进程 | **不在防御范围内**（SECURITY.md 明示）：密钥由 Windows DPAPI CurrentUser 保护，但同用户的恶意进程原则上能访问同用户能访问的一切 |
| 被攻破的 WebView 渲染进程 | 重点防御对象：能力白名单 + 安全面基线把它能做的事压到最小（见下） |
| 网络上的攻击者 | 所有模型服务请求走 HTTPS（rustls）；WebSocket 用 TLS + 系统根证书 |

**本项目没有任何服务器。** 请求只发往你在「服务」页配置的模型服务商。

## 密钥保管

- **存储**：API Key 写入 Windows 凭据管理器（Credential Manager），经 `keyring` crate 的 `windows-native-keyring-store` 后端（底层 DPAPI CurrentUser）。非 Windows 平台回退内存实现，仅用于测试。
- **配置文件里没有密钥**：JSON 配置只存密钥引用；界面读取的公开配置（`PublicConfig`）同样不含明文。
- **内存卫生**：内存中的密钥副本用 `zeroize` 的 `Zeroizing` 包装，用完即清零。
- **错误不泄密**：密钥相关错误只有两个稳定错误码（`SECRET_REFERENCE_INVALID` / `SECRET_BACKEND_UNAVAILABLE`），不带细节。

相关代码：[secrets/](../src-tauri/src/secrets/mod.rs)、[Cargo.toml](../src-tauri/src/Cargo.toml)（`keyring`、`zeroize` 钉版依赖）。

## Tauri 能力白名单

Tauri 2 的能力模型决定了渲染进程能调用哪些命令。RoleAI 的 [capabilities/main.json](../src-tauri/capabilities/main.json) 只声明：

- 作用域仅 `main` 窗口；
- `core:default` 基础能力 + **逐命令**的 `allow-*` 条目（会话、资料、直播、OBS 状态等约 70 条），没有通配的宽权限。

新增 Tauri 命令时必须同步加对应的最小权限条目——多给的权限过不了基线测试（见下节）。

## 安全面基线测试

`tests/tauri/security-surface-baseline.json` 记录了一组安全关键文件的 SHA-256（能力白名单、`lib.rs`、`contracts.rs`、`src/api/commands.ts`、生成的绑定、`tauri.conf.json` 等）。契约测试逐文件重算哈希，任何一个字节变了都会红测。

**这是刻意的"绊线"**：这些文件任何一个被改动（包括你以为无害的重排版），都必须有意识地重算哈希、和代码改动放同一个提交、并在评审里可见。它防的不是攻击者——攻击者改不了你的已提交基线——而是"顺手改了一行没人注意"的回归。

改动流程写在该 JSON 的 `comment` 字段里：用规定的单行命令重算哈希、更新 JSON、同一提交提交。

相关代码：[tests/tauri/security-surface-baseline.json](../tests/tauri/security-surface-baseline.json)、[tests/tauri/](../tests/tauri/)（其余契约测试：IPC 入口唯一、旧协议来源拒绝等）。

## 诊断导出的路径限制

诊断导出（`diagnostics-export`）会把脱敏后的诊断 JSON 写到用户指定路径。这个能力如果在被攻破的渲染进程手里，就是一个"任意位置写文件"的原语。因此写入口有路径校验：目的地必须是绝对路径，且其父目录（canonicalize 之后）必须位于应用数据目录内，否则返回稳定错误码 `DIAGNOSTICS_DESTINATION_INVALID`。该行为由真实 IPC 回归测试钉住（外部路径必拒、数据目录内必成）。

相关代码：[commands/system.rs](../src-tauri/src/commands/system.rs)（`ensure_diagnostics_destination`）、[commands_ipc_tests.rs](../src-tauri/src/commands_ipc_tests.rs)。

## 数据去向

| 数据 | 去向 | 说明 |
| --- | --- | --- |
| 麦克风音频（工作台对练） | 本机 → 你配置的 ASR / Realtime 服务 | WebView 采集，经本机 IPC 给主进程；PCM 只经本机 Tauri 事件传输，**不入库** |
| 会议进程音频 | 本机回环采集（AudioBridge） | WASAPI 进程环回，不经网络；白名单外的进程不能被采集 |
| 对话文本 / 资料 | 你配置的 LLM / Embedding / 联网搜索服务 | 检索命中的分块会随提示词发给 LLM；Embedding 请求发给对应服务 |
| API Key | Windows 凭据管理器；请求头里发给对应服务商 | 只发给你配置的那家；不发给任何第三方 |
| 会话记录 / 纪要 / 配置 | 本机 SQLite 与配置目录 | 备份产物是本机文件 |
| 诊断导出 | 本机文件（限定在数据目录内） | 导出内容为脱敏诊断 JSON |

## 给使用者的三条底线

1. **知情**：在会议或直播里使用 RoleAI 时，告知参与者 AI 参与和记录方式。
2. **人工复核**：AI 的输出由人复核后再作为对外内容或结论；预设角色的提示词已写明不作自动录用决定。
3. **不要外泄自己的数据**：提 Issue 或贴日志前先删除密钥、录音、转写与个人信息（[SECURITY.md](../SECURITY.md) 与 Issue 模板都会提醒）。

## 负责任披露

发现安全问题请通过 GitHub Security Advisory 私下报告（仓库 Security 标签页 → Report a vulnerability），附最小复现步骤。不要在公开 Issue 里贴 Key、Token 或候选人数据。
