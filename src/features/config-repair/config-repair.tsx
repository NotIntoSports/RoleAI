import type { PublicError, StartupState } from "../../generated/bindings";
import { t } from "../../i18n";

type RepairProps = {
  state: Extract<StartupState, { kind: "recoverable" | "invalid" }>;
  busy: boolean;
  onRestoreLastGood: () => void;
  onRestoreDefaults: () => void;
  onOpenConfig: () => void;
};

export function ConfigRepair({ state, busy, onRestoreLastGood, onRestoreDefaults, onOpenConfig }: RepairProps) {
  const error: PublicError = state.error;
  return (
    <main className="foundation-shell">
      <section className="foundation-card repair-card" aria-labelledby="repair-title">
        <p className="foundation-eyebrow">{t("repair.eyebrow")}</p>
        <h1 id="repair-title">{t("repair.heading")}</h1>
        <dl className="repair-error">
          <div><dt>{t("repair.errorCodeLabel")}</dt><dd>{error.code}</dd></div>
          {error.field ? <div><dt>{t("repair.fieldLabel")}</dt><dd>{error.field}</dd></div> : null}
          <div><dt>{t("repair.messageLabel")}</dt><dd>{error.message}</dd></div>
        </dl>
        <div className="repair-actions">
          {state.kind === "recoverable" ? (
            <button type="button" disabled={busy} onClick={onRestoreLastGood}>{t("repair.restoreLastGood")}</button>
          ) : null}
          <button type="button" disabled={busy} onClick={onRestoreDefaults}>{t("repair.restoreDefaults")}</button>
          <button type="button" disabled title={t("repair.importConfigTitle")}>{t("repair.importConfig")}</button>
          <button type="button" disabled={busy} onClick={onOpenConfig}>{t("repair.openConfig")}</button>
        </div>
      </section>
    </main>
  );
}
