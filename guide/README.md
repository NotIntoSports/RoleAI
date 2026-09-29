# RoleAI 文档

这里是 RoleAI 的公开文档。应用内的「服务」「设置」页负责日常配置；本目录回答「它是怎么工作的」和「出问题怎么查」。

## 目录

### 了解项目

- [架构说明](architecture.md) —— 模块职责、进程模型、IPC 与数据存储
- [实时语音管线深度解析](realtime-voice-pipeline.md) —— 断句、回声、打断、断线重连的设计与取舍

### 使用指南

- [配置指南](configuration.md) —— 服务商、语音线路、配置文件与环境变量
- [本地知识库与混合检索](knowledge-base.md) —— 资料导入、分块、检索打分与备份
- [故障排查](troubleshooting.md) —— 按现象查找原因与解决方法

### 设计与安全

- [安全设计与数据去向](security.md) —— 威胁模型、密钥保管、Tauri 能力白名单
- [架构决策记录（ADR）](adr/) —— 关键技术决策的背景、选项与后果

## 相关链接

- [中文 README](../README.md) · [English](../README.en.md)
- [第三方组件与许可证](../THIRD_PARTY_NOTICES.md)
- [安全策略与漏洞报告](../SECURITY.md)
- [参与贡献](../CONTRIBUTING.md)
