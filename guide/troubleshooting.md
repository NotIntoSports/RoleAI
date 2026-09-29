# 故障排查

按「现象 → 原因 → 解决」组织。这里只列应用里有对应错误码与处理逻辑的情况——界面上的报错文案与本文一致（错误码 → 文案的映射见 [workspace-format.ts](../src/features/session/workspace-format.ts)）。找不到你的问题？到设置页导出诊断信息，按 [SECURITY.md](../SECURITY.md) 的提醒清理敏感内容后提 Issue。

## 供应商与密钥

### 提示「本机未找到供应商密钥」（PROVIDER_CREDENTIAL_MISSING）

- **原因**：密钥引用存在但凭据管理器里没有对应条目——常见于换了 Windows 用户、迁移了配置文件、或手动清理过凭据管理器。
- **解决**：服务页 → 对应供应商 → 重新保存 API Key。

### 提示余额不足（错误码 1113）或模型权限错误（downstream_reconnect_exceeded）

- **原因**：透传自供应商。`1113` 是账号语音资源包耗尽；`downstream_reconnect_exceeded` 通常表示账号未开通对应实时语音模型权限。
- **解决**：到供应商控制台充值或开通模型权限；或在服务页更换语音线路。

### 短时间内反复失败

- **原因**：供应商限流（如阿里云实时模型对高频请求限流），界面会附加限流提示。
- **解决**：等待 1–2 分钟后再说话重试；不要连续快速重发加深限流。

## 实时语音（Realtime 端到端线路）

| 现象 | 错误码 | 原因与解决 |
| --- | --- | --- |
| 无法解析服务地址 | `REALTIME_DNS_FAILED` | 检查网络与 Base URL 拼写 |
| 无法建立 TCP 连接 | `REALTIME_TCP_FAILED` | 检查网络、代理与防火墙 |
| 安全连接失败 | `REALTIME_TLS_FAILED` | 检查系统时间是否正确、证书或代理是否拦截 TLS |
| 连接失败 | `REALTIME_CONNECT_FAILED` | 网络或供应商地址问题，检查服务页配置 |
| 协议协商失败 | `REALTIME_PROTOCOL_FAILED` | 服务地址不支持 Realtime 协议，确认填的是 Realtime 端点而不是普通 Chat API |
| 鉴权失败 | `REALTIME_UNAUTHORIZED` | 检查 API Key 是否为实时线路所用、套餐状态与模型权限 |
| 服务已断开 / 读写失败 | `REALTIME_CONNECTION_CLOSED` `REALTIME_READ_FAILED` `REALTIME_WRITE_FAILED` | 网络波动。Realtime 线路会自动指数退避重连（500 ms 起、30 s 封顶）并回放上下文；输入已保留 |
| 请求超时 | `REALTIME_TIMEOUT` `REALTIME_SESSION_UPDATE_TIMEOUT` | 供应商未及时响应或未确认会话；输入已保留，稍后重试 |
| 助手没有响应 | `REALTIME_NO_RESPONSE` | 供应商侧偶发。再点一次「让助手回答」（快捷键 Ctrl+Alt+A） |

## 会议音频与播放

### 提示「缺少 AudioBridge 音频组件」（SESSION_SIDECAR_MISSING）

- **原因**：安装包内的 AudioBridge.exe 缺失或损坏。
- **解决**：重新安装最新版；从源码运行的需要先构建 AudioBridge（见仓库 [native/AudioBridge/README.md](../native/AudioBridge/README.md)）。

### 提示「音频组件启动失败」（SESSION_SIDECAR_SPAWN_FAILED）

- **原因**：AudioBridge.exe 存在但无法启动（权限、杀软拦截、运行库缺失）。
- **解决**：检查杀软隔离区并放行；重装应用。

### 提示「请选择有效的会议进程」（SESSION_SIDECAR_INVALID_PID / MEETING_PROCESS_NOT_AVAILABLE）

- **原因**：所选会议已退出；或目标程序不在会议进程白名单内（白名单按可执行文件名精确匹配）。
- **解决**：刷新会议进程列表后重新选择；确认会议软件正在运行。

### 提示「语音未播放成功」（PLAYBACK_FAILED / PLAYBACK_START_FAILED / PLAYBACK_TIMEOUT）

- **原因**：所选音频设备不可用、被拔出或被独占；AudioBridge 未就绪。
- **解决**：设置页 → 音频路由里重新选择输出设备；文字回答已保留，不会丢。
- 注意：播报走指定设备，不占用系统默认设备；会议场景请把会议软件的麦克风设为虚拟声卡的采集端。

### 提示「音频组件未确认播放完成」（PLAYBACK_NOT_CONFIRMED）

- **原因**：播放进程在完成前异常退出，应用拒绝把该轮标记为已播报（防止把没播出去的内容当成说过）。
- **解决**：重试该轮；若反复出现，检查杀软对 AudioBridge 的拦截。

## 会话操作

### 提示「还没有可回答的发言」（NOTHING_TO_ANSWER）

- **原因**：当前没有定稿的用户发言可供回答。
- **解决**：说完一句、等状态栏显示转写定稿后再按「让助手回答」。

### 发送被取消（SESSION_CANCELLED）

- **原因**：你（或接管/停止操作）取消了本轮。
- **解决**：输入已保留，重新发送即可。

## 知识库

### 资料导入了但检索不到

- **原因**：资料还在解析/分块中，或 Embedding 未配置（此时只有关键词一路检索）。
- **解决**：资料页查看索引状态；配置 Embedding 后向量索引会自动补齐（见[本地知识库](knowledge-base.md)）。

### 更换 Embedding 服务后检索异常

- **原因**：正常情况下向量表按嵌入空间指纹自动重建；若中途失败会停在旧状态。
- **解决**：服务页重新测试并激活 Embedding 配置，触发重建。

## 配置与诊断

### 应用启动后配置是空的 / 配置位置不对

- **原因**：配置文件按优先级定位（`--config` → `AI_VIRTUAL_ASSISTANT_CONFIG` → 开发默认 → 安装默认），环境变量残留会让应用读到另一份配置。
- **解决**：设置页查看当前配置位置；清掉不用的 `AI_VIRTUAL_ASSISTANT_CONFIG` 环境变量。详见[配置指南](configuration.md)。

### 诊断导出被拒绝（DIAGNOSTICS_DESTINATION_INVALID）

- **原因**：导出目的地必须位于应用数据目录内（防止被用作任意路径写入）。
- **解决**：在设置页选择应用建议的导出位置。

### 需要看详细日志

- 设置环境变量 `RUST_LOG=debug` 后从终端启动（`npm run tauri:dev`），日志走 stdout；默认级别为 `info`。贴日志前先删密钥与个人信息。
