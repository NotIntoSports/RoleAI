import { FormEvent, useCallback, useEffect, useState } from "react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, PublicConfig } from "../../generated/bindings";
import { t, useT } from "../../i18n";

const emptyEmbedding = {
  id: "",
  providerId: "",
  baseUrl: "",
  apiKey: "",
  modelId: "",
  dimensions: "1536",
  normalized: true,
};

const optional = (value: string) => value.trim() || null;

export function EmbeddingEditor({ focusId = null }: { focusId?: string | null }) {
  useT();
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState(() => t("services.reading"));
  const [busy, setBusy] = useState(false);
  const [embedding, setEmbedding] = useState(emptyEmbedding);

  const reload = useCallback(async () => {
    try {
      const result = await api.getConfigPublic();
      if (result.ok) {
        setConfig(result.data);
        setMessage("");
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage(t("services.ipc.statusUnavailable"));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!focusId || !config) return;
    const item = config.knowledge.embeddingConfigs.find((entry) => entry.id === focusId);
    if (!item) {
      setEmbedding(emptyEmbedding);
      setMessage(t("services.embedding.notFound", { id: focusId }));
      return;
    }
    setEmbedding({
      id: item.id,
      providerId: item.providerId,
      baseUrl: item.baseUrl ?? "",
      apiKey: "",
      modelId: item.modelId,
      dimensions: String(item.dimensions),
      normalized: item.normalized,
    });
  }, [focusId, config]);

  async function run<T>(action: () => Promise<CommandResult<T>>, success: string) {
    setBusy(true);
    try {
      const result = await action();
      await reload();
      if (!result.ok) {
        setMessage(errorText(result.error));
        return result;
      }
      setMessage(success);
      return result;
    } catch {
      await reload();
      setMessage(t("services.ipc.operateFailed"));
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const usingProvider = Boolean(embedding.providerId.trim());
    try {
      const result = await run(
        () =>
          api.saveEmbeddingConfig({
            id: optional(embedding.id),
            providerId: embedding.providerId.trim(),
            baseUrl: usingProvider ? null : optional(embedding.baseUrl),
            apiKey: usingProvider ? null : optional(embedding.apiKey),
            modelId: embedding.modelId.trim(),
            dimensions: Number(embedding.dimensions),
            normalized: embedding.normalized,
          }),
        t("services.embedding.saved"),
      );
      if (result?.ok) {
        setEmbedding((current) => ({ ...current, id: result.data.id, apiKey: "" }));
        return;
      }
    } finally {
      setEmbedding((current) => ({ ...current, apiKey: "" }));
    }
  }

  const providers = config?.models.providers ?? [];
  const items = config?.knowledge.embeddingConfigs ?? [];
  const usingProvider = Boolean(embedding.providerId.trim());

  return (
    <section className="service-panel embedding-editor" aria-labelledby="embedding-editor-heading">
      <h2 className="section-heading" id="embedding-editor-heading">Embedding</h2>
      <p className="configuration-description">{t("services.embedding.description")}</p>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{embedding.id && items.some((item) => item.id === embedding.id) ? t("services.embedding.editTitle") : t("services.embedding.addTitle")}</h3>
          <label>
            {t("services.embedding.providerLabel")}
            <select
              value={embedding.providerId}
              onChange={(event) => setEmbedding({ ...embedding, providerId: event.target.value, apiKey: "" })}
            >
              <option value="">{t("services.embedding.providerOffOption")}</option>
              {providers.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name || t("services.providers.unnamed")}
                </option>
              ))}
            </select>
          </label>
          {!usingProvider && (
            <>
              <label>
                {t("services.embedding.endpointLabel")}
                <input
                  required
                  type="url"
                  placeholder="http://127.0.0.1:8080/v1"
                  value={embedding.baseUrl}
                  onChange={(event) => setEmbedding({ ...embedding, baseUrl: event.target.value })}
                />
              </label>
              <label>
                API Key
                <input
                  type="password"
                  autoComplete="new-password"
                  value={embedding.apiKey}
                  onChange={(event) => setEmbedding({ ...embedding, apiKey: event.target.value })}
                />
                <small>{t("services.embedding.apiKeyHint")}</small>
              </label>
            </>
          )}
          <label>
            {t("services.embedding.modelLabel")}
            <input
              required
              value={embedding.modelId}
              onChange={(event) => setEmbedding({ ...embedding, modelId: event.target.value })}
            />
          </label>
          <label>
            {t("services.embedding.dimensionsLabel")}
            <input
              required
              type="number"
              min={1}
              max={65536}
              value={embedding.dimensions}
              onChange={(event) => setEmbedding({ ...embedding, dimensions: event.target.value })}
            />
          </label>
          <label>
            {t("services.embedding.distanceLabel")}
            <input readOnly value="cosine" />
          </label>
          <label className="configuration-checkbox">
            <input
              type="checkbox"
              checked={embedding.normalized}
              onChange={(event) => setEmbedding({ ...embedding, normalized: event.target.checked })}
            />
            {t("services.embedding.normalizedLabel")}
          </label>
          <button className="button-primary" disabled={busy} type="submit">
            {t("services.embedding.save")}
          </button>
        </form>
        <div className="service-list configuration-list">
          <h3>{t("services.embedding.listHeading")} <span className="configuration-count">{items.length}</span></h3>
          {items.length === 0 && <EmptyState title={t("services.embedding.empty")} />}
          {items.map((item) => {
            const providerName = providers.find((provider) => provider.id === item.providerId)?.name;
            const source = providerName || item.baseUrl || t("services.embedding.customSource");
            return (
            <article className="service-card" key={item.id}>
              <h3>{item.modelId}</h3>
              <p>
                {source} · {t("services.embedding.dimensionUnit", { n: item.dimensions })} · cosine
              </p>
              <p>{item.ready ? t("services.embedding.ready") : t("services.embedding.notReady")}</p>
              {item.active && <span className="status-badge">{t("services.embedding.activeBadge")}</span>}
              <div className="service-actions">
                <button
                  aria-label={t("services.embedding.editAria", { name: item.modelId })}
                  disabled={busy}
                  onClick={() =>
                    setEmbedding({
                      id: item.id,
                      providerId: item.providerId,
                      baseUrl: item.baseUrl ?? "",
                      apiKey: "",
                      modelId: item.modelId,
                      dimensions: String(item.dimensions),
                      normalized: item.normalized,
                    })
                  }
                >
                  {t("services.embedding.edit")}
                </button>
                <button disabled={busy} onClick={() => void run(() => api.testEmbeddingConfig(item.id), t("services.embedding.testPassed"))}>
                  {t("services.embedding.test")}
                </button>
                <button
                  disabled={busy || !item.ready}
                  onClick={() => void run(() => api.activateEmbeddingConfig(item.id), t("services.embedding.activated"))}
                >
                  {t("services.embedding.activate")}
                </button>
                <button className="button-danger" disabled={busy} onClick={() => void run(() => api.deleteEmbeddingConfig(item.id), t("services.embedding.deleted"))}>
                  {t("services.embedding.delete")}
                </button>
              </div>
            </article>
            );
          })}
        </div>
      </div>
    </section>
  );
}
