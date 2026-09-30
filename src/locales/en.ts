// English copy. The key set must mirror zh-CN exactly;
// this is enforced at compile time via the Dictionary type.
import type { Dictionary } from "./zh-CN";

export const en: Dictionary = {
  settings: {
    appearance: {
      language: {
        legend: "Interface language",
        system: "System",
        simplifiedChinese: "简体中文",
        english: "English",
        note: "Applies immediately. The preference is saved on this device.",
      },
    },
  },
};

export default en;
