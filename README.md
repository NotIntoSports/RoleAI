# RoleAI

**让 AI 以你设定的角色，参与实时交流。**

RoleAI 是一款 Windows 桌面 AI 角色交互助手。你可以设定角色身份、开场白和表达风格，选择模型与语音服务，结合导入的资料进行语音对话。它可用于面试问答、会议辅助和直播讲解，并提供会话记录与直播讲稿管理。

角色配置、资料和会话记录保存在本机；语音识别、模型回答与语音合成由你配置的服务处理。

仓库里只有一条产品路径：**Tauri** 桌面客户端。不再提供 Electron、Next.js、登录、Control API 或 Python Agent。

> 当前版本是 Windows x64 内部试用版。使用前须告知对方 AI 参与和记录方式，并由人工复核；不得用于隐蔽冒充或未经复核的自动决策。

## 名称与兼容性

应用名称为 **RoleAI**，Windows 可执行文件为 `role-ai-desktop.exe`。为继续读取已有配置与数据，保留原有应用标识、`%APPDATA%\AI Virtual Assistant` 配置目录及 `AI_VIRTUAL_ASSISTANT_CONFIG` 环境变量。GitHub 仓库名与本地目录名不影响应用名称。

## 环境要求

- Windows x64
- Rust 1.96
- Node.js 24
- Microsoft Edge WebView2 Runtime

## 启动

```powershell
npm install
npm run tauri:dev
```

打包：

```powershell
npm run tauri:build
```

## 验证

```powershell
npm run test:tauri
npm run test:tauri-package
```

`test:tauri` 运行 Rust 测试、Tauri UI 测试、契约测试并构建前端。`test:tauri-package` 在隔离的临时配置目录中启动已打包的可执行文件，等待最多 15 秒确认主窗口可见，并断言进程树没有 Node、Go Control API、Python、PostgreSQL 或 Nginx，安装包目录也未混入本地配置、数据库、日志或凭据测试文件。完整打包冒烟需要已经构建好的 exe；日常 CI 不跑这一步。

## 当前能力

- 本机配置 + Windows 凭据保管（Credential Manager），无登录
- SQLite 资料库与会话记录
- Direct Runtime：级联（ASR → LLM → TTS）与端到端 Realtime
- 会话命令（播报、重试、修正、纪要等）
- 可选 LiveKit 传输（同一套会话机；默认不依赖）
- C# AudioBridge 仍用于会议进程音频采集
- 无 Control API、无 Python Agent、无 Electron / Next.js

页面：工作台、虚拟直播、资料、记录、服务、设置。

## 界面与外观

在「设置 → 外观」选择跟随系统、浅色或深色，偏好仅保存在本机。服务按模型供应商、语音线路、Embedding、LiveKit 分类；同一页切换分类会保留未提交的表单内容。工作台的朗读、纠正、重试与报告收纳在「会话工具」中。

界面复用现有 React 和原生控件，使用 `lucide-react@1.41.0` 的具名图标（ISC，部分继承图标含 MIT 声明）；无需额外配置、远程字体或图标服务。

## OBS 与虚拟摄像头

Phase 6 只完成了本机 OBS / AudioBridge 路径解析和前置探测。托管 OBS、虚拟摄像头启停和快捷键界面已延期；当前客户端不会创建场景、浏览器源或启动 Virtual Camera。不要用 Electron 去补完 OBS。

可把官方便携 OBS 放到 `resources/prerequisites` 供探测使用；当前 Tauri 安装包不会管理或随包启动 OBS。

## 当前限制

- 仅 Windows x64 内部试用，须告知对方并由人工复核。
- 托管 OBS / 虚拟摄像头 / 热键 UI 尚未接入。
- 设置页部分出镜与诊断能力仍是壳层占位。
- 级联线路（ASR / 非流式 LLM / TTS）遇到连接失败或 HTTP 429/502/503/504 会自动重试，最多 2 次（间隔 300ms/900ms）；超时、401/403、其余 4xx 与响应超限不重试。Realtime 断线不自动重连；需在界面手动重试或重新开始会话。
- 客户端不做请求速率限制，重复操作由界面防重复提交与后端互斥锁保护。
