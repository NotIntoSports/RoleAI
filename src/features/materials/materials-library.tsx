import { FormEvent, useCallback, useEffect, useState } from "react";
import { ChevronDown, FileText, FolderOpen, RefreshCw, Search, Trash2, Upload } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, MaterialSearchHit, MaterialSummary } from "../../generated/bindings";
import "../../styles/library.css";

const materialStatus: Record<string, string> = {
  text_ready: "文本就绪",
  vector_ready: "已建索引",
  failed: "处理失败",
};

export interface MaterialsLibraryProps {
  selectPath?: () => Promise<string | null>;
}

export function MaterialsLibrary({ selectPath }: MaterialsLibraryProps) {
  const [items, setItems] = useState<MaterialSummary[]>([]);
  const [hits, setHits] = useState<MaterialSearchHit[]>([]);
  const [message, setMessage] = useState("正在读取本地资料…");
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
      setMessage("IPC_UNAVAILABLE：无法读取本地资料");
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
      setMessage("IPC_UNAVAILABLE：本地操作失败");
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function submitImport(event: FormEvent) {
    event.preventDefault();
    await run(() => api.importMaterial(path.trim()), "资料已导入");
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
      setMessage("IPC_UNAVAILABLE：本地操作失败");
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
        <h2 id="materials-library-heading">资料库</h2>
        {loaded && <span className="muted">{items.length} 份资料</span>}
      </div>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="library-toolbar">
        <form className="service-form library-search" role="search" aria-label="搜索资料" onSubmit={submitSearch}>
          <label className="library-search-field">
            <span className="library-sr-only">检索词</span>
            <Search size={16} aria-hidden="true" />
            <input value={query} placeholder="搜索资料内容…" onChange={(event) => setQuery(event.target.value)} />
          </label>
          <button className="button-primary" disabled={busy} type="submit">
            搜索
          </button>
        </form>
        <button
          className="button-ghost"
          disabled={busy}
          type="button"
          onClick={() => void run(() => api.indexMaterials(), "索引已重建")}
        >
          <RefreshCw size={16} aria-hidden="true" />
          重建索引
        </button>
      </div>
      <details className="library-import">
        <summary>
          <Upload size={16} aria-hidden="true" />
          导入资料
          <ChevronDown className="library-disclosure-icon" size={16} aria-hidden="true" />
        </summary>
        <form className="service-form library-import-form" onSubmit={submitImport}>
          <label>
            文件路径
            <input type="text" value={path} placeholder="输入本地文件的完整路径" onChange={(event) => setPath(event.target.value)} />
          </label>
          <div className="service-actions">
            {selectPath && (
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void pickPath()}>
                <FolderOpen size={16} aria-hidden="true" />
                选择文件
              </button>
            )}
            <button className="button-primary" disabled={busy} type="submit">
              导入
            </button>
          </div>
        </form>
      </details>
      {hasSearched && (
        <section className="library-results" aria-labelledby="material-results-heading">
          <div className="library-heading">
            <h3 id="material-results-heading">检索结果</h3>
            <span className="muted">{hits.length} 个片段</span>
          </div>
          {hits.length === 0 && <EmptyState title="未找到匹配内容，试试其他关键词。" />}
          {hits.map((item) => (
            <article className="library-search-hit" key={item.chunkId}>
              <div className="library-hit-source">
                <FileText size={15} aria-hidden="true" />
                <span className="muted">来源</span>
                <h4>{item.fileName}</h4>
                {item.section && <span className="muted">{item.section}</span>}
              </div>
              <p className="library-snippet">{item.snippet}</p>
            </article>
          ))}
        </section>
      )}
      <div className="library-rows" aria-label="已导入资料" aria-busy={busy}>
        {loaded && items.length === 0 && (
          <EmptyState
            className="library-empty"
            icon={<FolderOpen size={28} aria-hidden="true" />}
            title="还没有资料。"
            hint="导入本地文件，让助手参考你的资料回答。"
          />
        )}
        {items.map((item) => (
          <article className="library-row" key={item.id} aria-label={item.fileName}>
            <div className="library-file-icon"><FileText size={20} aria-hidden="true" /></div>
            <div className="library-row-content">
              <h3>{item.fileName}</h3>
              <div className="library-meta">
                <span className="status-badge" data-tone={item.status === "failed" ? "danger" : "neutral"}>
                  {materialStatus[item.status] ?? item.status}
                </span>
                <span>{item.chunkCount} 个切片</span>
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
                  void run(() => api.deleteMaterial(item.id), "资料已删除").then(() => {
                    setPendingDelete(null);
                  });
                }}
              >
                <Trash2 size={15} aria-hidden="true" />
                {pendingDelete === item.id ? "确认删除" : "删除"}
              </button>
              {pendingDelete === item.id && (
                <button className="button-ghost" disabled={busy} type="button" onClick={() => setPendingDelete(null)}>
                  取消
                </button>
              )}
            </div>
          </article>
        ))}
      </div>
    </section>
  );
}
