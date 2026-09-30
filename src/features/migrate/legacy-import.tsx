import { FormEvent, useState } from "react";

import { importLegacySource } from "../../api/commands";
import { t, useT } from "../../i18n";

export function LegacyImportPanel() {
  useT();
  const [path, setPath] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      const result = await importLegacySource(path.trim());
      if (result.ok) {
        setMessage(t("migrate.legacyImport.imported", { sessions: result.data.sessions, turns: result.data.turns }));
      } else {
        setMessage(`${result.error.code}：${result.error.message}`);
      }
    } catch {
      setMessage(t("migrate.legacyImport.ipcFailed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="service-panel settings-section" aria-labelledby="legacy-import-heading">
      <h2 className="section-heading" id="legacy-import-heading">{t("migrate.legacyImport.heading")}</h2>
      <p className="configuration-description">{t("migrate.legacyImport.description")}</p>
      <form className="service-form configuration-migration-form" onSubmit={(event) => void submit(event)}>
        <label htmlFor="legacy-source-path">{t("migrate.legacyImport.pathLabel")}</label>
        <input
          id="legacy-source-path"
          name="legacySourcePath"
          value={path}
          onChange={(event) => setPath(event.target.value)}
          placeholder={t("migrate.legacyImport.pathPlaceholder")}
        />
        <button className="button-primary" type="submit" disabled={busy || path.trim().length === 0}>
          {t("migrate.legacyImport.action")}
        </button>
      </form>
      {message ? <p className="services-message" role="status">{message}</p> : null}
    </section>
  );
}
