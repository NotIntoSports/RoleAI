import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, ArrowUpRight, Download, MessageSquare, Quote, Trash2 } from "lucide-react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, SessionDetail, SessionSummary } from "../../generated/bindings";
import "../../styles/library.css";

const sessionStatus: Record<string, string> = {
  idle: "未开始",
  preparing: "准备中",
  listening: "聆听中",
  thinking: "思考中",
  speaking: "朗读中",
  paused: "已暂停",
  stopping: "结束中",
  completed: "已完成",
  recovering: "恢复中",
  blocked: "需处理",
  failed: "失败",
  interrupted: "已中断",
};

function sessionDate(session: SessionSummary) {
  const value = session.startedAt ?? session.updatedAt;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value || "时间未知";
  return date.toLocaleString("zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23",
  });
}

function roleLabel(roleProfileId: string, names: Record<string, string>) {
  const id = roleProfileId.trim();
  if (!id) return "未指定";
  return names[id] || "已删除角色";
}

export function RecordsList() {
  const [items, setItems] = useState<SessionSummary[]>([]);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [message, setMessage] = useState("正在读取会话记录…");
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
        setMessage("IPC_UNAVAILABLE：无法读取会话记录");
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
      setMessage("IPC_UNAVAILABLE：无法读取会话记录");
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
      setMessage("IPC_UNAVAILABLE：本地操作失败");
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
      setMessage("IPC_UNAVAILABLE：本地操作失败");
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
      setMessage(format === "csv" ? `延迟数据已导出：${result.data.path}` : `时间线已导出（Perfetto / chrome://tracing 可打开）：${result.data.path}`);
    } catch {
      setMessage("IPC_UNAVAILABLE：本地操作失败");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="service-panel records-list" aria-labelledby="records-list-heading">
      <div className="library-heading">
        <h2 id="records-list-heading">会话记录</h2>
        {loaded && !detail && <span className="muted">{items.length} 次会话</span>}
      </div>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      {detail ? (
        <section className="record-detail" aria-label="会话详情" aria-busy={busy}>
          <div className="record-detail-toolbar">
            <button className="button-ghost" disabled={busy} type="button" onClick={() => setDetail(null)}>
              <ArrowLeft size={16} aria-hidden="true" />
              返回列表
            </button>
            <div className="service-actions" aria-label="导出会话">
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void exportRecord("markdown")}>
                <Download size={15} aria-hidden="true" />
                导出 Markdown
              </button>
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void exportRecord("json")}>
                导出 JSON
              </button>
              <button className="button-ghost" disabled={busy} type="button" onClick={() => void exportRecord("text")}>
                导出文本
              </button>
              <button
                className="button-ghost"
                disabled={busy}
                type="button"
                aria-label="导出延迟数据 CSV"
                onClick={() => void exportLatency("csv")}
              >
                导出延迟数据
              </button>
              <button
                className="button-ghost"
                disabled={busy}
                type="button"
                aria-label="导出时间线 Chrome Trace JSON"
                onClick={() => void exportLatency("trace")}
              >
                导出时间线
              </button>
            </div>
          </div>
          <header className="record-detail-heading">
            <div className="library-heading">
              <h3><time dateTime={detail.session.startedAt ?? detail.session.updatedAt}>{sessionDate(detail.session)}</time></h3>
              <span className="status-badge" data-tone={detail.session.status === "failed" ? "danger" : "neutral"}>
                {sessionStatus[detail.session.status] ?? detail.session.status}
              </span>
            </div>
            <p className="library-meta">角色 {roleLabel(detail.session.roleProfileId, roleNames)}</p>
            <p className="record-id">会话 ID <code>{detail.session.id}</code></p>
          </header>
          {detail.turns.length === 0 && <EmptyState title="本次会话还没有对话内容。" />}
          {detail.turns.map((item) => (
            <article className="record-turn" key={item.id} aria-label={`回合 ${item.turnIndex + 1}`}>
              <h3>回合 {item.turnIndex + 1}</h3>
              <section className="record-message record-user" aria-label="用户内容">
                <h4>你</h4>
                <p>{item.userText || "本轮没有用户内容。"}</p>
              </section>
              <section className="record-message record-assistant" aria-label="AI 回复">
                <h4>AI 助手</h4>
                <p>{item.assistantText || "本轮没有 AI 回复。"}</p>
              </section>
              <section className="record-citations" aria-label="资料引用">
                {!item.materialsUsed && <p className="muted">本轮未使用资料</p>}
                {item.citations.length > 0 && <h4><Quote size={14} aria-hidden="true" />引用片段</h4>}
                {item.citations.map((citation) => (
                  <blockquote key={`${citation.materialId}-${citation.chunkId}`}>
                    <p>{citation.snippet}</p>
                    <footer>资料 ID <code>{citation.materialId}</code></footer>
                  </blockquote>
                ))}
              </section>
            </article>
          ))}
        </section>
      ) : (
        <div className="library-rows" aria-label="会话列表" aria-busy={busy}>
          {loaded && items.length === 0 && (
            <EmptyState
              className="library-empty"
              icon={<MessageSquare size={28} aria-hidden="true" />}
              title="还没有记录。"
              hint="在工作台开始会话后，可在这里回看对话。"
            />
          )}
          {items.map((item) => (
            <article className="library-row" key={item.id}>
              <div className="library-file-icon"><MessageSquare size={20} aria-hidden="true" /></div>
              <div className="library-row-content">
                <h3><time dateTime={item.startedAt ?? item.updatedAt}>{sessionDate(item)}</time></h3>
                <div className="library-meta">
                  <span className="status-badge" data-tone={item.status === "failed" ? "danger" : "neutral"}>
                    {sessionStatus[item.status] ?? item.status}
                  </span>
                  <span>角色 {roleLabel(item.roleProfileId, roleNames)}</span>
                </div>
                <p className="record-id">会话 ID <code>{item.id}</code></p>
              </div>
              <div className="service-actions library-row-actions">
                <button className="button-ghost" disabled={busy} type="button" onClick={() => void openDetail(item.id)}>
                  查看
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
                    void run(() => api.deleteSession(item.id), "记录已删除").then(() => {
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
      )}
    </section>
  );
}
