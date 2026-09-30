import { useEffect, useState } from "react";

import { getStartupState, openAppDirectory, restoreDefaultConfig, restoreLastGoodConfig } from "../api/commands";
import type { StartupState } from "../generated/bindings";
import { t } from "../i18n";
import { ConfigRepair } from "../features/config-repair/config-repair";
import { Shell } from "./shell";

export function App() {
  const [startup, setStartup] = useState<StartupState | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void getStartupState().then((result) => {
      if (result.ok) setStartup(result.data);
      else setStartup({ kind: "invalid", error: result.error });
    }).catch(() => {
      setStartup({ kind: "invalid", error: { code: "STARTUP_STATE_UNAVAILABLE", message: t("app.startup.serviceStateUnavailable"), requestId: "local", retryable: false } });
    });
  }, []);

  async function repair(action: typeof restoreDefaultConfig) {
    setBusy(true);
    const result = await action();
    setStartup(result.ok ? result.data : { kind: "invalid", error: result.error });
    setBusy(false);
  }

  if (!startup) {
    return <main className="foundation-shell"><p role="status">{t("app.startup.checking")}</p></main>;
  }
  if (startup.kind === "recoverable" || startup.kind === "invalid") {
    return <ConfigRepair state={startup} busy={busy} onRestoreLastGood={() => void repair(restoreLastGoodConfig)} onRestoreDefaults={() => void repair(restoreDefaultConfig)} onOpenConfig={() => void openAppDirectory("config")} />;
  }
  return <Shell />;
}
