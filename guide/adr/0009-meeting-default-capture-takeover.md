# 0009 — 会议会话期间接管系统默认麦克风（三角色）

- 状态：已采纳
- 日期：2026-10-01

## 背景

ADR 0006 立下关键约束“不改系统默认设备”，随后为会议模式开了第一个口子：会话启动时把 Windows“默认通信设备”（`Role.Communications`）切到 CABLE Output，结束时还原。但实测多数会议软件（腾讯会议、Zoom、飞书、钉钉）跟随的是“默认设备”（`eConsole` / `eMultimedia` 角色）而非“默认通信设备”，用户仍要在会议软件里手动选一次麦克风。

Windows 没有按进程指定采集设备的标准 API（按应用选设备仅限播放端）；改会议软件私有配置文件或做 UI 自动化都脆弱且越界。

## 选项

1. **维持仅切通信角色 + 文案引导**——用户诉求（免手动选择）不解决。
2. **会话期间把 CABLE Output 设为全部三个角色的默认采集端点，结束按角色还原**——业界标准做法（SoundVolumeView、Audio Switcher 等均用 `IPolicyConfig::SetDefaultEndpoint` 按角色切换）。
3. **按会议软件改配置文件 / UI 自动化**——每个应用、每个版本都要维护，越界且脆弱。

## 决策

选项 2。扩展 AudioBridge（ADR 0006 的既有 COM 调用，零新增依赖）：

- `--set-default-capture-mic <captureId>`：三角色一次性接管，输出各角色会话前设备 JSON；部分角色失败时自动还原已切换的角色。
- `--restore-default-capture-mic <cableId> <consoleTarget> <multimediaTarget> <communicationsTarget>`：按角色还原，护栏保留——当前值仍是 CABLE 才还原，不覆盖用户中途手改的选择；空目标跳过。
- 恢复路径五条：会话停止、启动回滚、会话失败（本轮补上，此前 fail 不还原）、应用退出（Drop）、应用强杀后下次启动（audio-routing.json 恢复）。

ADR 0006 的“不改系统默认设备”约束由此界定：会议会话期间临时接管全部默认采集角色，会话结束还原；默认**播放**设备仍然不动（AI 语音按 endpoint id 直接播到 CABLE Input）。

## 后果

- 好处：跟随系统默认设备的会议软件全自动工作，无需手动选择；会话期间系统行为可预期（所有应用默认麦克风一致指向虚拟线路，物理麦克风可经“人工接管模式”混入同一线路）。
- 代价：会话期间其他用默认麦克风的应用临时收不到物理麦克风（结束还原）；未跟随默认设备的应用首次仍需手动选一次 CABLE Output（一次性，端点 ID 稳定会记住）；`IPolicyConfig` 仍是未公开 COM 接口，Windows 大版本更新需回归验证（沿用 0006 已接受的风险）。
