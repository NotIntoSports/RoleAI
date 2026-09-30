import { useEffect, type ReactNode } from "react";

import { initializeLanguage } from "./language";

export function I18nProvider({ children }: { children: ReactNode }) {
  // StrictMode 下 effect 会挂载/卸载各一次，initializeLanguage 每次调用返回对应 dispose。
  useEffect(() => initializeLanguage(), []);
  return <>{children}</>;
}
