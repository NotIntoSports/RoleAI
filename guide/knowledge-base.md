# 本地知识库与混合检索

RoleAI 的「资料」页是一个完全本地的知识库：简历、岗位 JD、产品文档导进去，会话里的 AI 回答就能引用这些内容。本文讲它的四个环节：导入解析、分块、混合检索、备份恢复。全部数据存本机 SQLite，推理所需的向量也在本机算、本机存。

相关页面：资料（导入、索引状态、检索预览）；架构背景见[架构说明](architecture.md)。

## 1. 资料导入与解析

**支持的格式**：PDF、DOCX、纯文本。

- PDF 用 `pdf-extract`（0.12.0）解析；
- DOCX 用 `docx-rs`（0.4.22）解析，遍历段落、表格与结构化标签，表格单元格按行拼接；
- 解析有预算控制（时间上限），超限返回稳定错误码，不会卡住界面。

**去重**：每份资料按内容的 SHA-256 建指纹，重复导入同一份文件直接返回已有记录的 ID，磁盘上永远只有一份副本（有测试钉住这个语义）。

**解析器版本化**：每条资料记录解析器版本与分块器版本。未来升级解析算法时，旧资料可以被识别出来按需重建，而不是全体静默重跑。

相关代码：[materials/parse.rs](../src-tauri/src/materials/parse.rs)、[services/materials.rs](../src-tauri/src/services/materials.rs)（`find_by_hash` 去重）。

## 2. 分块策略

分块器版本 `resume-semantic-v1`，为简历与文档类内容设计：

- **按章节标题切**：识别常见章节标题（教育经历、工作经历、项目经验等），优先在语义边界切分；
- **单块上限 2000 字符**：超长的段落按窗口硬切，回看 400 字符避免句子在边界被腰斩；
- **最多 500 块**：单份资料的分块数量有硬上限，防止异常文档拖垮索引。

每块记录来源资料、章节名、起止字符位置，检索命中后可以定位回原文做证据引用。

相关代码：[materials/chunk.rs](../src-tauri/src/materials/chunk.rs)（常量与版本号在文件头部）。

## 3. 混合检索：FTS + 向量 + RRF

**问题。** 只用关键词检索，问"带队经验"找不到写着"担任技术负责人"的段落；只用向量检索，人名、产品名、数字这类精确词容易被语义漂移稀释。

**方案。** 两路并行，倒数排序融合（RRF）：

1. **FTS5 全文检索**：SQLite 内建 FTS5 虚拟表（`material_chunks_fts`），对查询词做关键词匹配，取前 20 条候选；
2. **向量检索**：资料分块经你配置的 Embedding 服务向量化后存入 sqlite-vec 的 vec0 虚拟表（`material_chunk_vectors`）；查询时把查询文本同样向量化，做近邻检索，取前 20 条候选；
3. **RRF 融合**：每路候选按名次打分 `1 / (60 + rank)`，同一块的得分相加后排序。名次融合对两路分数的量纲不敏感——不需要调"关键词 0.4、向量 0.6"这种很难解释的权重。

**一致性细节。**

- **嵌入空间指纹**：向量表按 Embedding 配置的指纹（供应商、模型、维度、是否归一化）管理。换了 Embedding 服务，维度对不上会自动重建向量表，不会拿两套空间的向量互相检索。
- **状态门槛**：只有 `text_ready` / `vector_ready` 状态的资料参与检索；正在删除的资料先标记 `deleting` 并屏蔽检索，再物理删除。
- **向量可用性**：Embedding 是可选项——没配 Embedding 服务时 FTS 一路照常工作，配了之后向量一路自动补索引。

相关代码：[materials/hybrid.rs](../src-tauri/src/materials/hybrid.rs)（`search_hybrid`、`reciprocal_rank_fusion`，`CANDIDATE_K = 20`、`RRF_K = 60.0`）、[database/mod.rs](../src-tauri/src/database/mod.rs)（sqlite-vec 注册）、[migrations/0002_materials.sql](../src-tauri/migrations/0002_materials.sql)（表结构）。

## 4. 检索结果怎么用

检索命中的分块连同章节、来源一起注入会话提示词，AI 回答时能引用具体出处；「记录」页的纪要把证据引用一并落库，可回看可导出。检索本身发生在主进程内，查询与结果不出本机——只有发给 Embedding / LLM 服务的请求内容遵循[安全设计与数据去向](security.md)所述的规则。

## 5. 备份与恢复

- **备份**：用 SQLite 在线备份 API 把数据库完整复制到归档目录（`archive.sqlite`），不需要停会话；
- **恢复**：先写回滚文件（`.restore-rollback`）与恢复日志（`.restore-journal.json`）再执行恢复——恢复中途失败可以回滚到恢复前状态，不会把现有数据库切成两半。

备份产物是标准 SQLite 文件，可以用任何 SQLite 工具打开检查。

相关代码：[materials/backup.rs](../src-tauri/src/materials/backup.rs)。

## 操作速查

| 想做什么 | 在哪里做 |
| --- | --- |
| 导入简历 / JD / 产品文档 | 资料页 → 导入（PDF / DOCX / 文本） |
| 看分块与索引状态 | 资料页 → 资料详情（分块列表、向量状态） |
| 验证检索效果 | 资料页 → 检索预览（命中块 + 融合得分） |
| 备份 / 恢复 | 设置页 → 数据（归档目录、恢复） |
| 更换 Embedding 服务 | 服务页 → Embedding（向量表自动重建） |
