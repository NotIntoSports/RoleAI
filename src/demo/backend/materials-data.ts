// 演示资料库种子：3 份完全虚构的资料（内容仅供在线演示检索与引用展示）。
export interface DemoMaterialSection {
  title: string;
  text: string;
}

export interface DemoMaterialDoc {
  id: string;
  fileName: string;
  mediaType: string;
  byteSize: number;
  status: string;
  chunkCount: number;
  contentSha256: string;
  sections: DemoMaterialSection[];
}

export const DEMO_MATERIALS: DemoMaterialDoc[] = [
  {
    id: "mat-demo-handbook",
    fileName: "云帆协同办公平台产品手册 v2.3.pdf",
    mediaType: "application/pdf",
    byteSize: 1187424,
    status: "indexed",
    chunkCount: 42,
    contentSha256: "demo-9f21c7a4handbook",
    sections: [
      {
        title: "产品概述",
        text: "云帆协同办公平台（虚构产品）面向 50～5000 人规模的企业，提供消息、文档、日程与审批的一体化协作。v2.3 重点加强了跨部门项目空间与离线消息同步，客户端覆盖 Windows、macOS 与浏览器。",
      },
      {
        title: "消息与协作",
        text: "消息服务支持单聊、群聊与频道三种形态，单条消息上限 32MB，支持已读回执与引用回复。离线消息在服务端保留 90 天，重新上线后按会话增量拉取。会议助手可以回答会议中@助手的提问，并依据本地上传的资料作答。",
      },
      {
        title: "文档协同",
        text: "在线文档支持多人实时编辑与历史版本回溯，历史版本保留 180 天。表格支持常见函数与数据透视。文档可以一键转为评审任务并指派给团队成员，评审意见会汇总到项目空间。",
      },
      {
        title: "安全与合规",
        text: "传输层全链路 TLS 1.3，存储层按租户隔离加密。管理员可以配置水印、外发审批与设备白名单。审计日志保留 365 天，支持导出给 SIEM 平台。v2.3 的客户端崩溃率（虚构指标）为 0.12%，较 v2.2 下降约 40%。",
      },
      {
        title: "版本记录",
        text: "v2.3（2026-09）新增项目空间与离线同步改进；v2.2（2026-06）新增会议助手与审计导出；v2.1（2026-03）重构了移动端消息列表。已知限制：离线模式下文档编辑暂不支持表格函数自动重算。",
      },
    ],
  },
  {
    id: "mat-demo-article",
    fileName: "高并发缓存的三板斧：击穿、穿透、雪崩.md",
    mediaType: "text/markdown",
    byteSize: 14210,
    status: "indexed",
    chunkCount: 18,
    contentSha256: "demo-3b7e11ccarticle",
    sections: [
      {
        title: "缓存击穿",
        text: "缓存击穿指单个热点 key 过期的瞬间，大量并发请求同时回源数据库。常用方案有两种：一是互斥重建（singleflight），只放一个请求持锁回源，其余请求等待或返回旧值；二是逻辑过期，key 物理上不过期，值里带过期时间，发现过期后由异步线程重建。重建线程要设置锁超时与重试上限，避免死锁。",
      },
      {
        title: "缓存穿透",
        text: "缓存穿透指查询根本不存在的数据，请求每次都打到数据库。防御手段：布隆过滤器在缓存之前拦截肯定不存在的 key；对空结果做短 TTL 的空值缓存（例如 60 秒）；入口参数做合法性校验。布隆过滤器要按数据量预估误判率并定期重建。",
      },
      {
        title: "缓存雪崩",
        text: "缓存雪崩指大量 key 在同一时间集中过期，或缓存实例宕机导致流量全部压到数据库。防御手段：过期时间加随机抖动；构建多级缓存（本地缓存 + 分布式缓存）；对回源链路做限流与降级预案；缓存集群做高可用部署。",
      },
      {
        title: "缓存一致性",
        text: "常见的缓存一致性策略是先更新数据库再删除缓存（Cache Aside）。对一致性要求更高的场景，可以订阅 binlog（例如 Canal）异步删除缓存，把强一致降级为最终一致，并用较短的 TTL 兜底。主从延迟期间要避免读到从库旧数据后回填缓存。",
      },
    ],
  },
  {
    id: "mat-demo-jd",
    fileName: "后端工程师岗位JD（云帆科技）.pdf",
    mediaType: "application/pdf",
    byteSize: 88064,
    status: "indexed",
    chunkCount: 6,
    contentSha256: "demo-55aa0d12jd",
    sections: [
      {
        title: "岗位职责",
        text: "负责协同办公平台消息与协同链路的服务端设计与研发；参与高并发场景（消息推送、缓存、存储）的容量规划与稳定性建设；与客户端、测试协作完成功能交付与线上问题复盘。",
      },
      {
        title: "任职要求",
        text: "三年以上后端开发经验；熟悉 Java 或 Go；熟悉 Redis、MySQL、Kafka 等常用组件的原理与调优；理解分布式系统常见问题（一致性、幂等、限流、降级）；有缓存体系设计经验者优先；良好的沟通与文档习惯。",
      },
    ],
  },
];
