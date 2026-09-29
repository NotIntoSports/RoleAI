import { useEffect, useState } from "react";
import { Activity, ArchiveRestore, Palette, UserRound } from "lucide-react";

import { getDiagnosticsLatencySummary, getLegacyMigrationStatus } from "../../api/commands";
import { AppearanceSettings } from "../../features/appearance/appearance-settings";
import { DiagnosticsPanel } from "../../features/diagnostics/diagnostics-panel";
import { LegacyImportPanel } from "../../features/migrate/legacy-import";
import { ReenterSecretsBanner } from "../../features/migrate/reenter-secrets-banner";
import { RoleEditor } from "../../features/roles/role-editor";
import type { LegacyMigrationStatus } from "../../generated/bindings";
import { PageShell } from "../page-shell";
import "../../styles/configuration.css";

const categories = [
  { id: "appearance", label: "外观", icon: Palette },
  { id: "roles", label: "角色", icon: UserRound },
  { id: "migration", label: "数据迁移", icon: ArchiveRestore },
  { id: "diagnostics", label: "诊断", icon: Activity },
] as const;

export function SettingsPage() {
  const [migration, setMigration] = useState<LegacyMigrationStatus | null>(null);
  const [category, setCategory] = useState<(typeof categories)[number]["id"]>(() => {
    const requested = new URLSearchParams(window.location.search).get("category");
    return categories.find((item) => item.id === requested)?.id ?? "appearance";
  });

  useEffect(() => {
    void getLegacyMigrationStatus()
      .then((result) => {
        if (result.ok) setMigration(result.data);
      })
      .catch(() => {
        setMigration(null);
      });
  }, []);

  return (
    <div className="settings-page">
      <PageShell id="settings" />
      <ReenterSecretsBanner status={migration} />
      <div className="configuration-layout">
        <nav className="settings-nav" aria-label="设置分类">
          {categories.map(({ id, label, icon: Icon }) => (
            <button key={id} id={`settings-category-${id}`} type="button" aria-current={category === id ? "page" : undefined} aria-controls={`settings-panel-${id}`} onClick={() => setCategory(id)}>
              <Icon size={16} aria-hidden="true" />{label}
            </button>
          ))}
        </nav>
        <div className="configuration-content">
          <section id="settings-panel-appearance" hidden={category !== "appearance"} aria-labelledby="settings-category-appearance"><AppearanceSettings /></section>
          <section id="settings-panel-roles" hidden={category !== "roles"} aria-labelledby="settings-category-roles"><RoleEditor /></section>
          <section id="settings-panel-migration" hidden={category !== "migration"} aria-labelledby="settings-category-migration"><LegacyImportPanel /></section>
          <section id="settings-panel-diagnostics" hidden={category !== "diagnostics"} aria-labelledby="settings-category-diagnostics">
            {category === "diagnostics" ? <DiagnosticsPanel loadSummary={getDiagnosticsLatencySummary} /> : null}
          </section>
        </div>
      </div>
    </div>
  );
}
