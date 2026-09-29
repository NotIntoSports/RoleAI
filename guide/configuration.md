# 配置指南

日常配置都在应用内的「服务」「设置」页完成，不需要手改配置文件。本文说明各处配置的含义、配置文件的定位规则，以及给高级用户的文件级说明。

## 配置的三层入口

| 层级 | 位置 | 适合 |
| --- | --- | --- |
| 应用界面（推荐） | 服务页（供应商 / 语音线路 / Embedding / 凭据）、设置页（外观、音频、导出目录） | 绝大多数场景 |
| 配置文件 | 见下文定位规则 | 批量修改、备份恢复、进阶调整 |
| 环境变量 / 命令行 | `AI_VIRTUAL_ASSISTANT_CONFIG`、`--config`、`RUST_LOG` | 多份配置切换、开发调试 |

## 配置文件定位（按优先级）

1. 命令行 `--config <路径>`；
2. 环境变量 `AI_VIRTUAL_ASSISTANT_CONFIG`（文件或目录路径均可）；
3. 开发模式：仓库内 `config/local.json`；
4. 正式安装：`%APPDATA%\AI Virtual Assistant\config.json`。

设置页会显示当前使用的配置位置；「恢复上次良好配置」「恢复默认」也在设置页。模板见仓库的 [config/local.example.json](../config/local.example.json)。

历史兼容：配置目录沿用旧名 `AI Virtual Assistant`，这是刻意的——升级安装后能直接读到旧配置与数据（见 README「名称与兼容性」）。

## 配置文件字段速查

顶层结构与常用字段（完整模板见 `config/local.example.json`）：

| 字段 | 含义 |
| --- | --- |
| `configVersion` | 配置结构版本号，迁移用，不要手改 |
| `application.locale` | 界面语言区域（当前 `zh-CN`） |
| `models.providers[]` | 模型供应商列表：`id`、`name`、`baseUrl`（OpenAI 兼容根地址）、`credential` |
| `models.providers[].credential` | **只有引用**（`reference`）与 `configured` 标记——明文 Key 永远在 Windows 凭据管理器里，不落在配置文件 |
| `models.activeProviderId` | 当前激活的供应商 |
| `speech.voiceRoutes[]` | 语音线路：`mode`（`cascaded` 级联）、ASR/LLM/TTS 各自的 `providerId` + `modelId`、`voiceId` 音色 |
| `speech.activeVoiceRouteId` | 当前激活线路 |
| `knowledge.embeddingConfigs[]` | Embedding 配置：供应商、模型、`dimensions`、`distance`（如 `cosine`）、`normalized` |
| `roleProfiles[]` | 角色档案：`name`、`systemPrompt`、`openingMessage`、`styleInstructions` |
| `activeRoleProfileId` | 当前激活角色 |
| `storage.exportDirectory` | 记录导出目录 |
| `diagnostics.logRetentionDays` | 诊断日志保留天数 |

改配置文件前先备份；结构不对时应用会拒绝加载并提示，不会静默吞掉。

## 服务怎么填（界面流程）

1. **供应商**：服务页 → 供应商 → 新建。填名称和 Base URL（OpenAI 兼容根地址，如 `https://api.example.com/v1`），保存 API Key（进凭据管理器），点「测试」验证连通。也可以从预设供应商列表一键填好。
2. **语音线路**：服务页 → 语音线路。级联模式下 ASR / LLM / TTS 三段各自选供应商与模型，可混搭；端到端 Realtime 选对应 Realtime 模型。保存后点「测试」。
3. **Embedding（可选）**：服务页 → Embedding。不配也能用（知识库只走关键词检索）；配了之后向量检索自动补索引。维度以服务商标注为准。
4. **声音复刻（可选）**：服务页 → 音色。上传参考音频创建音色，把 `voiceId` 配到线路里。

## 日志与诊断

- **日志级别**：默认 `info`；设置环境变量 `RUST_LOG=debug` 打开全量调试日志。日志走 stdout，`npm run tauri:dev` 的终端里可见。
- **诊断导出**：设置页 → 诊断导出，产出脱敏 JSON，只能写到应用数据目录内（见[安全设计](security.md)）。
- **日志保留**：`diagnostics.logRetentionDays` 控制保留天数（默认 14）。

提 Issue 前请先删除日志里的密钥与个人信息（见 [SECURITY.md](../SECURITY.md)）。

## 相关页面

- 装不上、连不通？→ [故障排查](troubleshooting.md)
- 密钥与数据流向 → [安全设计与数据去向](security.md)
- 资料库与 Embedding 的关系 → [本地知识库与混合检索](knowledge-base.md)
