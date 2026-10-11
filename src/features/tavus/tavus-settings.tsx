import { FormEvent, useCallback, useEffect, useState } from "react";

import * as api from "../../api/commands";
import { errorNoticeText as errorText } from "../../components/error-notice";
import { t, useT } from "../../i18n";

export function TavusSettings() {
  useT();
  const [configured, setConfigured] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const result = await api.getTavusCredentialStatus();
      if (result.ok) setConfigured(result.data.configured);
    } catch {
      setConfigured(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      const result = await api.saveTavusCredential(apiKey.trim());
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      setApiKey("");
      setConfigured(result.data.configured);
      setMessage(result.data.configured ? t("tavus.saved") : t("tavus.cleared"));
    } catch {
      setMessage(t("roles.ipc.operateFailed"));
    } finally {
      setBusy(false);
    }
  }

  async function listFaces() {
    setBusy(true);
    try {
      const result = await api.listTavusFaces();
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      const names = result.data.slice(0, 8).map((face) => face.name).join("、");
      setMessage(t("tavus.faceCount", { n: result.data.length, names }));
    } catch {
      setMessage(t("roles.ipc.operateFailed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="service-panel" aria-labelledby="tavus-settings-heading">
      <h2 className="section-heading" id="tavus-settings-heading">{t("tavus.heading")}</h2>
      <p className="configuration-description">{t("tavus.description")}</p>
      <p className="services-message" role="status">
        {configured ? t("tavus.configured") : t("tavus.missing")}
        {message ? ` ${message}` : ""}
      </p>
      <form className="service-form" onSubmit={save}>
        <label>
          {t("tavus.keyLabel")}
          <input
            type="password"
            autoComplete="off"
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
          />
        </label>
        <div className="service-actions">
          <button className="button-primary" disabled={busy} type="submit">{t("tavus.save")}</button>
          <button type="button" disabled={busy} onClick={() => void listFaces()}>{t("tavus.listFaces")}</button>
        </div>
      </form>
    </section>
  );
}
