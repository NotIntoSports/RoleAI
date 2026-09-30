// 简体中文文案基线（默认回退语言）。
// 规则：
// - 只允许字符串与嵌套对象（禁止数组、函数），键结构按页面/功能分组；
// - en.ts 必须与键集合完全一致（Dictionary 类型编译期约束 + tests/tauri/i18n-contract.test.mjs 运行期约束）；
// - 插值占位符使用 {name} 形式，参数通过 t(key, params) 传入。
export const zhCN = {
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
