// 简体中文文案基线（默认回退语言）。
// 规则：
// - 只允许字符串与嵌套对象（禁止数组、函数），键结构按页面/功能分组；
// - en.ts 必须与键集合完全一致（Dictionary 类型编译期约束 + tests/tauri/i18n-contract.test.mjs 运行期约束）；
// - 插值占位符使用 {name} 形式，参数通过 t(key, params) 传入。
export const zhCN = {
  app: {
    startup: {
      checking: "正在检查本地配置…",
      serviceStateUnavailable: "无法读取桌面服务状态",
    },
    nav: {
      primary: "主导航",
      toggle: "切换导航",
      localWorkspace: "本地工作空间",
    },
    routes: {
      labels: {
        workspace: "工作台",
        livestream: "虚拟直播",
        practice: "模拟面试",
        materials: "资料",
        records: "记录",
        services: "服务",
        settings: "设置",
      },
      descriptions: {
        workspace: "让对话自然进行，重要时刻由你掌控。",
        livestream: "根据产品资料准备讲稿，并通过 OBS 输出可控的虚拟直播。",
        practice: "按题单与 AI 面试官对练，结束后生成评分报告。",
        materials: "整理参考资料，为每一次回答提供上下文。",
        records: "回顾对话，留存值得继续跟进的内容。",
        services: "连接模型与语音服务，配置你的 AI 能力。",
        settings: "调整工作空间，管理角色与本地数据。",
      },
      capabilities: {
        workspace: [
          "当前会话状态与控制（启动/暂停/恢复/停止）",
          "AI 实时回复与字幕显示",
          "人工接管与干预控制",
          "音视频连接状态指示",
          "会议桥接卡片",
        ],
        livestream: [
          "本地产品资料生成分段讲稿",
          "图片或循环视频舞台",
          "讲稿确认与分段控制",
          "OBS Virtual Camera 输出",
        ],
        practice: [
          "岗位方向与 JD/简历资料选择",
          "面试官风格选择（面试官/HR/严苛面试官）",
          "题量、难度与预计时长配置",
          "题单预览、编辑与保存",
        ],
        materials: [
          "简历导入与管理",
          "知识库切片与索引状态",
          "FTS 全文检索",
          "向量嵌入（sqlite-vec）",
          "资料预览与筛选",
        ],
        records: [
          "会话记录列表与分页",
          "纪要详情（摘要/优势/跟进/局限/证据）",
          "记录导出",
          "记录删除（两步确认）",
        ],
        services: [
          "模型提供方配置与状态",
          "语音路由配置与测试",
          "密钥管理（Windows Credential Manager）",
          "连接测试与健康检查",
        ],
        settings: [
          "系统诊断导出",
          "配置位置选择与显示",
          "会议画面输出模式",
          "助手声音与形象",
          "OBS 与虚拟摄像头",
          "音频路由与设备检查",
        ],
      },
    },
  },
  settings: {
    appearance: {
      language: {
        legend: "界面语言",
        system: "跟随系统",
        simplifiedChinese: "简体中文",
        english: "English",
        note: "语言切换立即生效，偏好保存在本机。",
      },
    },
  },
} as const;

// 供各语言使用的词典结构：键集合与嵌套结构必须与 zhCN 完全一致，值放宽为 string。
// 直接用 typeof zhCN 会把中文文案定成字面量类型，其他语言词典无法赋不同文案。
type WidenValues<T> = { [K in keyof T]: T[K] extends string ? string : WidenValues<T[K]> };
export type Dictionary = WidenValues<typeof zhCN>;
export default zhCN;
