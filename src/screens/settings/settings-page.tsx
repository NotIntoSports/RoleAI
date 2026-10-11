import { useEffect, useState } from "react";
import { Activity, ArchiveRestore, Palette, RefreshCw, UserRound, Video } from "lucide-react";

import { getDiagnosticsLatencySummary, getLegacyMigrationStatus } from "../../api/commands";
import { AppearanceSettings } from "../../features/appearance/appearance-settings";
import { DiagnosticsPanel } from "../../features/diagnostics/diagnostics-panel";
import { LegacyImportPanel } from "../../features/migrate/legacy-import";
import { ReenterSecretsBanner } from "../../features/migrate/reenter-secrets-banner";
import { RoleEditor } from "../../features/roles/role-editor";
import { isServiceCategory, serviceCategories, ServiceSettings } from "../../features/services/service-settings";
import { TavusSettings } from "../../features/tavus/tavus-settings";
import { UpdaterPanel } from "../../features/updater/updater-panel";
import type { LegacyMigrationStatus } from "../../generated/bindings";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import { PageShell } from "../page-shell";
import "../../styles/configuration.css";

const categories = [
  ...serviceCategories,
  { id: "tavus", labelKey: "settings.page.categories.tavus", icon: Video },
  { id: "roles", labelKey: "settings.page.categories.roles", icon: UserRound },
  { id: "appearance", labelKey: "settings.page.categories.appearance", icon: Palette },
  { id: "updater", labelKey: "settings.page.categories.updater", icon: RefreshCw },
  { id: "migration", labelKey: "settings.page.categories.migration", icon: ArchiveRestore },
  { id: "diagnostics", labelKey: "settings.page.categories.diagnostics", icon: Activity },
] as const satisfies ReadonlyArray<{ id: string; labelKey: DictionaryStringKey; icon: typeof Palette }>;

// 分类按「AI 服务 / 工作空间 / 应用」三组排列，这两个是后两组的第一项。
const groupStarts: ReadonlySet<string> = new Set(["roles", "updater"]);

export function SettingsPage() {
  useT();
  const [migration, setMigration] = useState<LegacyMigrationStatus | null>(null);
  const [category, setCategory] = useState<(typeof categories)[number]["id"]>(() => {
    const requested = new URLSearchParams(window.location.search).get("category");
    return categories.find((item) => item.id === requested)?.id ?? "providers";
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
      <ReenterSecretsBanner status={migration} onOpenProviders={() => setCategory("providers")} />
      <div className="configuration-layout">
        <nav className="settings-nav" aria-label={t("settings.page.navAria")}>
          {categories.map(({ id, labelKey, icon: Icon }) => (
            <button key={id} id={`settings-category-${id}`} type="button" data-group-start={groupStarts.has(id) || undefined} aria-current={category === id ? "page" : undefined} aria-controls={`settings-panel-${id}`} onClick={() => setCategory(id)}>
              <Icon size={16} aria-hidden="true" />{t(labelKey)}
            </button>
          ))}
        </nav>
        <div className="configuration-content">
          <ServiceSettings category={isServiceCategory(category) ? category : null} onCategoryChange={setCategory} />
          <section id="settings-panel-tavus" hidden={category !== "tavus"} aria-labelledby="settings-category-tavus"><TavusSettings /></section>
          <section id="settings-panel-roles" hidden={category !== "roles"} aria-labelledby="settings-category-roles"><RoleEditor visible={category === "roles"} onOpenTavus={() => setCategory("tavus")} /></section>
          <section id="settings-panel-appearance" hidden={category !== "appearance"} aria-labelledby="settings-category-appearance"><AppearanceSettings /></section>
          <section id="settings-panel-updater" hidden={category !== "updater"} aria-labelledby="settings-category-updater"><UpdaterPanel /></section>
          <section id="settings-panel-migration" hidden={category !== "migration"} aria-labelledby="settings-category-migration"><LegacyImportPanel /></section>
          <section id="settings-panel-diagnostics" hidden={category !== "diagnostics"} aria-labelledby="settings-category-diagnostics">
            {category === "diagnostics" ? <DiagnosticsPanel loadSummary={getDiagnosticsLatencySummary} /> : null}
          </section>
        </div>
      </div>
    </div>
  );
}
