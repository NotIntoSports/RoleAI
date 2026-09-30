import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, ArrowUpRight, Download, MessageSquare, Quote, Trash2 } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import { t, useLanguage, useT } from "../../i18n";
import type { CommandResult, SessionDetail, SessionSummary } from "../../generated/bindings";
import "../../styles/library.css";

const RECORD_STATUS_KEYS = [
  "idle", "preparing", "listening", "thinking", "speaking", "paused", "stopping",
  "completed", "recovering", "blocked", "failed", "interrupted",
] as const;
type KnownRecordStatus = (typeof RECORD_STATUS_KEYS)[number];

function recordStatusLabel(status: string): string {
  return (RECORD_STATUS_KEYS as readonly string[]).includes(status)
    ? t(`session.recordStatus.${status as KnownRecordStatus}`)
    : status;
}

function sessionDate(session: SessionSummary, language: string) {
  const value = session.startedAt ?? session.updatedAt;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value || t("session.records.unknownTime");
  return date.toLocaleString(language === "en" ? "en-US" : "zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23",
  });
}

function roleLabel(roleProfileId: string, names: Record<string, string>) {
  const id = roleProfileId.trim();
  if (!id) return t("session.records.unspecifiedRole");
  return names[id] || t("session.records.deletedRole");
}

export function RecordsList() {
  useT();
  const language = useLanguage();
  const [items, setItems] = useState<SessionSummary[]>([]);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [message, setMessage] = useState(() => t("session.records.reading"));
  const [busy, setBusy] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [roleNames, setRoleNames] = useState<Record<string, string>>({});

  const reload = useCallback(async () => {
    try {
      const [sessionsResult, configResult] = await Promise.allSettled([
        api.listSessions(),
        api.getConfigPublic(),
      ]);
      if (configResult.status === "fulfilled" && configResult.value.ok) {
        setRoleNames(
          Object.fromEntries(
            configResult.value.data.roleProfiles.map((profile) => [profile.id, profile.name]),
          ),
        );
      }
      if (sessionsResult.status !== "fulfilled") {
        setMessage(t("session.ipc.recordsUnavailable"));
        return;
      }
      if (sessionsResult.value.ok) {
        setItems(sessionsResult.value.data);
        setLoaded(true);
        setMessage("");
      } else {
        setMessage(errorText(sessionsResult.value.error));
      }
    } catch {
      setMessage(t("session.ipc.recordsUnavailable"));
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
      setMessage(t("session.ipc.operateFailed"));
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function openDetail(id: string) {
    setBusy(true);
    try {
      const result = await api.getSession(id);
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      setDetail(result.data);
      setMessage("");
    } catch {
      setMessage(t("session.ipc.operateFailed"));
    } finally {
      setBusy(false);
    }
  }

  async function exportRecord(format: "markdown" | "json" | "text") {
    if (!detail) return;
    setBusy(true);
    try {
      const result = await api.exportSession(detail.session.id, format);
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      setMessage(result.data.path);
    } catch {
      setMessage(t("session.ipc.operateFailed"));
    } finally {
      setBusy(false);
    }
  }

  async function exportLatency(format: "csv" | "trace") {
    if (!detail) return;
    setBusy(true);
    try {
      const result = await api.exportSessionLatency(detail.session.id, format);
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      setMessage(format === "csv" ? t("session.records.latencyExported", { path: result.data.path }) : t("session.records.traceExported", { path: result.data.path }));
    } catch {
      setMessage(t("session.ipc.operateFailed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="service-panel records-list" aria-labelledby="records-list-heading">
      <div className="library-heading">
        <h2 id="records-list-heading">{t("session.records.heading")}</h2>
        {loaded && !detail && <span className="muted">{t("session.records.countLabel", { n: items.length })}</span>}
      </div>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      {detail ? (
        <section className="record-detail" aria-label={t("session.records.detailRegion")} aria-busy={busy}>
          <div className="record-detail-toolbar">
            <button className="button-ghost" disabled={busy} type="button" onClick={() => setDetail(null)}>
              <ArrowLeft size={16} aria-hidden="true" />
              {t("session.records.backToList")}
            </button>
            <div className="service-actions" aria-label={t("session.records.exportGroup")}>
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void exportRecord("markdown")}>
                <Download size={15} aria-hidden="true" />
                {t("session.records.exportMarkdown")}
              </button>
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void exportRecord("json")}>
                {t("session.records.exportJson")}
              </button>
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void exportRecord("text")}>
                {t("session.records.exportText")}
              </button>
              <button
                className="button-ghost"
                disabled={busy}
                type="button"
                aria-label={t("session.records.latencyCsvAria")}
                onClick={() => void exportLatency("csv")}
              >
                {t("session.records.latencyCsv")}
              </button>
              <button
                className="button-ghost"
                disabled={busy}
                type="button"
                aria-label={t("session.records.traceAria")}
                onClick={() => void exportLatency("trace")}
              >
                {t("session.records.trace")}
              </button>
            </div>
          </div>
          <header className="record-detail-heading">
            <div className="library-heading">
              <h3><time dateTime={detail.session.startedAt ?? detail.session.updatedAt}>{sessionDate(detail.session, language)}</time></h3>
              <span className="status-badge" data-tone={detail.session.status === "failed" ? "danger" : "neutral"}>
                {recordStatusLabel(detail.session.status)}
              </span>
            </div>
            <p className="library-meta">{t("session.records.rolePrefix", { name: roleLabel(detail.session.roleProfileId, roleNames) })}</p>
            <p className="record-id">{t("session.records.sessionPrefix", { id: detail.session.id })} <code>{detail.session.id}</code></p>
          </header>
          {detail.turns.length === 0 && <EmptyState title={t("session.records.emptyTurns")} />}
          {detail.turns.map((item) => (
            <article className="record-turn" key={item.id} aria-label={t("session.records.turnLabel", { n: item.turnIndex + 1 })}>
              <h3>{t("session.records.turnLabel", { n: item.turnIndex + 1 })}</h3>
              <section className="record-message record-user" aria-label={t("session.records.userAria")}>
                <h4>{t("session.records.userHeading")}</h4>
                <p>{item.userText || t("session.records.userEmpty")}</p>
              </section>
              <section className="record-message record-assistant" aria-label={t("session.records.assistantAria")}>
                <h4>{t("session.records.assistantHeading")}</h4>
                <p>{item.assistantText || t("session.records.assistantEmpty")}</p>
              </section>
              <section className="record-citations" aria-label={t("session.records.citationsAria")}>
                {!item.materialsUsed && <p className="muted">{t("session.records.noMaterials")}</p>}
                {item.citations.length > 0 && <h4><Quote size={14} aria-hidden="true" />{t("session.records.citationsHeading")}</h4>}
                {item.citations.map((citation) => (
                  <blockquote key={`${citation.materialId}-${citation.chunkId}`}>
                    <p>{citation.snippet}</p>
                    <footer>{t("session.records.sourcePrefix", { id: citation.materialId })} <code>{citation.materialId}</code></footer>
                  </blockquote>
                ))}
              </section>
            </article>
          ))}
        </section>
      ) : (
        <div className="library-rows" aria-label={t("session.records.listAria")} aria-busy={busy}>
          {loaded && items.length === 0 && (
            <EmptyState
              className="library-empty"
              icon={<MessageSquare size={28} aria-hidden="true" />}
              title={t("session.records.emptyTitle")}
              hint={t("session.records.emptyHint")}
            />
          )}
          {items.map((item) => (
            <article className="library-row" key={item.id}>
              <div className="library-file-icon"><MessageSquare size={20} aria-hidden="true" /></div>
              <div className="library-row-content">
                <h3><time dateTime={item.startedAt ?? item.updatedAt}>{sessionDate(item, language)}</time></h3>
                <div className="library-meta">
                  <span className="status-badge" data-tone={item.status === "failed" ? "danger" : "neutral"}>
                    {recordStatusLabel(item.status)}
                  </span>
                  <span>{t("session.records.rolePrefix", { name: roleLabel(item.roleProfileId, roleNames) })}</span>
                </div>
                <p className="record-id">{t("session.records.sessionPrefix", { id: item.id })} <code>{item.id}</code></p>
              </div>
              <div className="service-actions library-row-actions">
                <button className="button-ghost" disabled={busy} type="button" onClick={() => void openDetail(item.id)}>
                  {t("session.records.view")}
                  <ArrowUpRight size={15} aria-hidden="true" />
                </button>
                <button
                  className={pendingDelete === item.id ? "button-danger" : "button-ghost"}
                  disabled={busy}
                  type="button"
                  onClick={() => {
                    if (pendingDelete !== item.id) {
                      setPendingDelete(item.id);
                      return;
                    }
                    void run(() => api.deleteSession(item.id), t("session.records.deleteSuccess")).then(() => {
                      setPendingDelete(null);
                    });
                  }}
                >
                  <Trash2 size={15} aria-hidden="true" />
                  {pendingDelete === item.id ? t("session.records.confirmDelete") : t("session.records.delete")}
                </button>
                {pendingDelete === item.id && (
                  <button className="button-ghost" disabled={busy} type="button" onClick={() => setPendingDelete(null)}>
                    {t("session.records.cancel")}
                  </button>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
