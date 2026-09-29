import { FormEvent, useCallback, useEffect, useState } from "react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, PublicConfig } from "../../generated/bindings";

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
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState("正在读取本地配置…");
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
      setMessage("IPC_UNAVAILABLE：无法读取本地配置");
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
      setMessage(`找不到 Embedding 配置（${focusId}），请重新选择。`);
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
      setMessage("IPC_UNAVAILABLE：本地操作失败");
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
        "Embedding 配置已保存，请先测试再启用",
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
      <p className="configuration-description">可以选用已保存的供应商，或自行填写 OpenAI 兼容接入地址。测试、切片和查询文本会发到该接口，不会经过作者服务器。</p>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{embedding.id && items.some((item) => item.id === embedding.id) ? "编辑配置" : "添加配置"}</h3>
          <label>
            供应商
            <select
              value={embedding.providerId}
              onChange={(event) => setEmbedding({ ...embedding, providerId: event.target.value, apiKey: "" })}
            >
              <option value="">不使用供应商，自行填写地址</option>
              {providers.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name || "未命名供应商"}
                </option>
              ))}
            </select>
          </label>
          {!usingProvider && (
            <>
              <label>
                接入地址
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
                <small>留空会保留已保存的密钥；无鉴权可留空</small>
              </label>
            </>
          )}
          <label>
            模型
            <input
              required
              value={embedding.modelId}
              onChange={(event) => setEmbedding({ ...embedding, modelId: event.target.value })}
            />
          </label>
          <label>
            维度
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
            距离
            <input readOnly value="cosine" />
          </label>
          <label className="configuration-checkbox">
            <input
              type="checkbox"
              checked={embedding.normalized}
              onChange={(event) => setEmbedding({ ...embedding, normalized: event.target.checked })}
            />
            归一化向量
          </label>
          <button className="button-primary" disabled={busy} type="submit">
            保存 Embedding
          </button>
        </form>
        <div className="service-list configuration-list">
          <h3>已配置模型 <span className="configuration-count">{items.length}</span></h3>
          {items.length === 0 && <EmptyState title="还没有 Embedding 配置。" />}
          {items.map((item) => {
            const providerName = providers.find((provider) => provider.id === item.providerId)?.name;
            const source = providerName || item.baseUrl || "自定义";
            return (
            <article className="service-card" key={item.id}>
              <h3>{item.modelId}</h3>
              <p>
                {source} · {item.dimensions} 维 · cosine
              </p>
              <p>{item.ready ? "测试通过" : "尚未就绪"}</p>
              {item.active && <span className="status-badge">当前启用</span>}
              <div className="service-actions">
                <button
                  aria-label={"编辑 " + item.modelId}
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
                  编辑
                </button>
                <button disabled={busy} onClick={() => void run(() => api.testEmbeddingConfig(item.id), "Embedding 测试通过")}>
                  测试
                </button>
                <button
                  disabled={busy || !item.ready}
                  onClick={() => void run(() => api.activateEmbeddingConfig(item.id), "Embedding 已启用")}
                >
                  启用
                </button>
                <button className="button-danger" disabled={busy} onClick={() => void run(() => api.deleteEmbeddingConfig(item.id), "Embedding 已删除")}>
                  删除
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
