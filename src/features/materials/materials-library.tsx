import { FormEvent, useCallback, useEffect, useState } from "react";
import { ChevronDown, FileText, FolderOpen, RefreshCw, Search, Trash2, Upload } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, MaterialSearchHit, MaterialSummary } from "../../generated/bindings";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import "../../styles/library.css";

const materialStatusKeys: Record<string, DictionaryStringKey> = {
  text_ready: "materials.status.text_ready",
  vector_ready: "materials.status.vector_ready",
  failed: "materials.status.failed",
};

export interface MaterialsLibraryProps {
  selectPath?: () => Promise<string | null>;
}

export function MaterialsLibrary({ selectPath }: MaterialsLibraryProps) {
  useT();
  const [items, setItems] = useState<MaterialSummary[]>([]);
  const [hits, setHits] = useState<MaterialSearchHit[]>([]);
  const [message, setMessage] = useState(() => t("materials.reading"));
  const [busy, setBusy] = useState(false);
  const [path, setPath] = useState("");
  const [query, setQuery] = useState("");
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [hasSearched, setHasSearched] = useState(false);

  const reload = useCallback(async () => {
    try {
      const result = await api.listMaterials();
      if (result.ok) {
        setItems(result.data);
        setLoaded(true);
        setMessage("");
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage(t("materials.ipc.listFailed"));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function run(action: () => Promise<CommandResult<unknown>>, success: string) {
    setBusy(true);
    try {
      const result = await action();
      await reload();
      if (!result.ok) {
        setMessage(errorText(result.error));
        return false;
      }
      setMessage(success);
      return true;
    } catch {
      await reload();
      setMessage(t("materials.ipc.operateFailed"));
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function submitImport(event: FormEvent) {
    event.preventDefault();
    await run(() => api.importMaterial(path.trim()), t("materials.imported"));
  }

  async function submitSearch(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      const result = await api.searchMaterials(query.trim());
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      setHits(result.data);
      setHasSearched(true);
      setMessage("");
    } catch {
      setMessage(t("materials.ipc.operateFailed"));
    } finally {
      setBusy(false);
    }
  }

  async function pickPath() {
    if (!selectPath) return;
    const picked = await selectPath();
    if (picked) setPath(picked);
  }

  return (
    <section className="service-panel materials-library" aria-labelledby="materials-library-heading">
      <div className="library-heading">
        <h2 id="materials-library-heading">{t("materials.heading")}</h2>
        {loaded && <span className="muted">{t("materials.countSuffix", { n: items.length })}</span>}
      </div>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="library-toolbar">
        <form className="service-form library-search" role="search" aria-label={t("materials.searchFormAria")} onSubmit={submitSearch}>
          <label className="library-search-field">
            <span className="library-sr-only">{t("materials.searchFieldLabel")}</span>
            <Search size={16} aria-hidden="true" />
            <input value={query} placeholder={t("materials.searchPlaceholder")} onChange={(event) => setQuery(event.target.value)} />
          </label>
          <button className="button-primary" disabled={busy} type="submit">
            {t("materials.searchAction")}
          </button>
        </form>
        <button
          className="button-ghost"
          disabled={busy}
          type="button"
          onClick={() => void run(() => api.indexMaterials(), t("materials.indexRebuilt"))}
        >
          <RefreshCw size={16} aria-hidden="true" />
          {t("materials.rebuildIndex")}
        </button>
      </div>
      <details className="library-import">
        <summary>
          <Upload size={16} aria-hidden="true" />
          {t("materials.importSummary")}
          <ChevronDown className="library-disclosure-icon" size={16} aria-hidden="true" />
        </summary>
        <form className="service-form library-import-form" onSubmit={submitImport}>
          <label>
            {t("materials.pathLabel")}
            <input type="text" value={path} placeholder={t("materials.pathPlaceholder")} onChange={(event) => setPath(event.target.value)} />
          </label>
          <div className="service-actions">
            {selectPath && (
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void pickPath()}>
                <FolderOpen size={16} aria-hidden="true" />
                {t("materials.pickFile")}
              </button>
            )}
            <button className="button-primary" disabled={busy} type="submit">
              {t("materials.importAction")}
            </button>
          </div>
        </form>
      </details>
      {hasSearched && (
        <section className="library-results" aria-labelledby="material-results-heading">
          <div className="library-heading">
            <h3 id="material-results-heading">{t("materials.resultsHeading")}</h3>
            <span className="muted">{t("materials.hitsCount", { n: hits.length })}</span>
          </div>
          {hits.length === 0 && <EmptyState title={t("materials.noHits")} />}
          {hits.map((item) => (
            <article className="library-search-hit" key={item.chunkId}>
              <div className="library-hit-source">
                <FileText size={15} aria-hidden="true" />
                <span className="muted">{t("materials.sourceLabel")}</span>
                <h4>{item.fileName}</h4>
                {item.section && <span className="muted">{item.section}</span>}
              </div>
              <p className="library-snippet">{item.snippet}</p>
            </article>
          ))}
        </section>
      )}
      <div className="library-rows" aria-label={t("materials.listAria")} aria-busy={busy}>
        {loaded && items.length === 0 && (
          <EmptyState
            className="library-empty"
            icon={<FolderOpen size={28} aria-hidden="true" />}
            title={t("materials.emptyTitle")}
            hint={t("materials.emptyHint")}
          />
        )}
        {items.map((item) => (
          <article className="library-row" key={item.id} aria-label={item.fileName}>
            <div className="library-file-icon"><FileText size={20} aria-hidden="true" /></div>
            <div className="library-row-content">
              <h3>{item.fileName}</h3>
              <div className="library-meta">
                <span className="status-badge" data-tone={item.status === "failed" ? "danger" : "neutral"}>
                  {materialStatusKeys[item.status] ? t(materialStatusKeys[item.status]) : item.status}
                </span>
                <span>{t("materials.chunkCount", { n: item.chunkCount })}</span>
              </div>
            </div>
            <div className="service-actions library-row-actions">
              <button
                className={pendingDelete === item.id ? "button-danger" : "button-ghost"}
                disabled={busy}
                type="button"
                onClick={() => {
                  if (pendingDelete !== item.id) {
                    setPendingDelete(item.id);
                    return;
                  }
                  void run(() => api.deleteMaterial(item.id), t("materials.deleted")).then(() => {
                    setPendingDelete(null);
                  });
                }}
              >
                <Trash2 size={15} aria-hidden="true" />
                {pendingDelete === item.id ? t("materials.confirmDelete") : t("materials.delete")}
              </button>
              {pendingDelete === item.id && (
                <button className="button-ghost" disabled={busy} type="button" onClick={() => setPendingDelete(null)}>
                  {t("materials.cancel")}
                </button>
              )}
            </div>
          </article>
        ))}
      </div>
    </section>
  );
}
