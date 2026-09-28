import { FormEvent, useCallback, useEffect, useState } from "react";
import { AudioLines, Boxes, Mic, Server } from "lucide-react";
import * as api from "../../api/commands";
import { EmbeddingEditor } from "../../features/services/embedding-editor";
import { VoiceReferenceEditor } from "../../features/services/voice-reference-editor";
import type { CommandResult, ProviderDependency, ProviderTestResult, PublicConfig, VoiceReferenceSummary, VoiceRouteMode, WebCapability } from "../../generated/bindings";
import { PageShell } from "../page-shell";
import "../../styles/configuration.css";

const optional = (value: string) => value.trim() || null;
const errorText = (error: { code: string; message: string; field?: string | null }) => ({
  PROVIDER_IN_USE: "供应商仍被配置引用，请先处理下方关联配置，再重试删除。",
  SECRET_CLEANUP_FAILED: "供应商配置已删除，但密钥清理失败。请点击重试清理密钥；不要重新添加供应商。",
  CONFIG_WRITE_FAILED: "配置写入失败，原配置未被删除，请检查目录写入权限。",
}[error.code] ?? error.message);
const providerTestMessage = (error: { code: string; message: string; field?: string | null }) => {
  switch (error.code) {
    case "PROVIDER_TIMEOUT": return "连接超时，请检查接入地址或网络";
    case "PROVIDER_REQUEST_FAILED": return "连接失败：供应商接口没有正常响应";
    case "PROVIDER_UNAUTHORIZED": return "连接失败：API Key 无效或没有权限";
    case "PROVIDER_ENDPOINT_INVALID": return "连接失败：接入地址无效";
    case "PROVIDER_RESPONSE_INVALID": return "已连通，但模型列表无法解析";
    case "PROVIDER_RESPONSE_TOO_LARGE": return "连接失败：供应商返回内容过大";
    case "PROVIDER_CLIENT_UNAVAILABLE": return "连接失败：本机无法发起请求";
    case "PROVIDER_NOT_FOUND": return "连接失败：找不到该供应商";
    default: return errorText(error);
  }
};
const providerTestSuccessMessage = (result: ProviderTestResult) => {
  const connection = `连接测试通过，发现 ${result.modelCount} 个模型`;
  switch (result.webStatus) {
    case "available": return `${connection}；联网可用，返回 ${result.webSourceCount} 个来源`;
    case "model_unsupported": return `${connection}；当前模型不支持联网`;
    case "interface_incompatible": return `${connection}；联网接口不兼容`;
    case "network_unreachable": return `${connection}；联网探测时网络不可达`;
    case "authentication_failed": return "鉴权失败：API Key 无效或没有联网权限";
    case "disabled": return `${connection}；未启用联网`;
  }
};
const initialRoute = { id: "", name: "", mode: "cascaded" as VoiceRouteMode, asrProviderId: "", asrModelId: "", llmProviderId: "", llmModelId: "", ttsProviderId: "", ttsModelId: "", voiceId: "", e2eProviderId: "", e2eModelId: "" };
type MessageTone = "info" | "pending" | "success" | "error";
type ProviderTestState = { tone: Exclude<MessageTone, "info">; text: string };
const categories = [
  { id: "providers", label: "模型供应商", icon: Server },
  { id: "routes", label: "语音线路", icon: AudioLines },
  { id: "voices", label: "音色克隆", icon: Mic },
  { id: "embedding", label: "Embedding", icon: Boxes },
] as const;

export function ServicesPage() {
  const [category, setCategory] = useState<(typeof categories)[number]["id"]>(() => {
    const requested = new URLSearchParams(window.location.search).get("category");
    return categories.find((item) => item.id === requested)?.id ?? "providers";
  });
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState("正在读取本地配置…");
  const [messageTone, setMessageTone] = useState<MessageTone>("pending");
  const [busy, setBusy] = useState(false);
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [providerTests, setProviderTests] = useState<Record<string, ProviderTestState>>({});
  const [provider, setProvider] = useState({ id: "", name: "", baseUrl: "", apiKey: "", webCapability: "none" as WebCapability });
  const [route, setRoute] = useState(initialRoute);
  const [deletion, setDeletion] = useState<{ id: string; name: string; references: ProviderDependency[]; cleanup?: boolean } | null>(null);
  const [embeddingFocusId, setEmbeddingFocusId] = useState<string | null>(null);
  const [voiceReferences, setVoiceReferences] = useState<VoiceReferenceSummary[]>([]);

  const refreshVoiceReferences = useCallback(async () => {
    try {
      const result = await api.listVoiceReferences();
      if (result.ok) setVoiceReferences(result.data);
    } catch { /* 音色 ID 建议加载失败时保持当前列表 */ }
  }, []);

  useEffect(() => {
    if (category === "routes" || category === "voices") void refreshVoiceReferences();
  }, [category, refreshVoiceReferences]);

  async function inspectDeletion(id: string, name: string) {
    setBusy(true);
    try {
      const result = await api.getModelProviderDependencies(id);
      if (!result.ok) { announce(errorText(result.error), "error"); return; }
      setDeletion({ id, name, references: result.data });
    } catch { announce("无法检查供应商引用，请重试。", "error"); }
    finally { setBusy(false); }
  }

  async function confirmDeletion() {
    if (!deletion) return;
    const target = deletion;
    const result = await run(() => api.deleteModelProvider(target.id), "供应商及密钥已删除");
    if (result?.ok || (result && !result.ok && result.error.code === "SECRET_CLEANUP_FAILED")) {
      if (provider.id === target.id) setProvider({ id: "", name: "", baseUrl: "", apiKey: "", webCapability: "none" });
      setModels((current) => { const next = { ...current }; delete next[target.id]; return next; });
      setProviderTests((current) => { const next = { ...current }; delete next[target.id]; return next; });
      setDeletion(result.ok ? null : { ...target, cleanup: true });
    } else if (result && !result.ok && result.error.code === "PROVIDER_IN_USE") {
      await inspectDeletion(target.id, target.name);
    }
  }

  function openReference(reference: ProviderDependency) {
    if (reference.kind === "voiceRoute") {
      const item = config?.speech.voiceRoutes.find((value) => value.id === reference.id);
      if (item) setRoute({ ...initialRoute, ...Object.fromEntries(Object.entries(item).filter(([key]) => key in initialRoute).map(([key, value]) => [key, value ?? ""])) } as typeof initialRoute);
      setCategory("routes");
    } else {
      setEmbeddingFocusId(reference.id);
      setCategory("embedding");
    }
    setDeletion(null);
    announce(`请处理关联配置「${reference.name}」（${reference.id}），完成后返回供应商重新删除。`, "info");
  }

  const announce = (text: string, tone: MessageTone) => {
    setMessage(text);
    setMessageTone(tone);
  };

  const reload = useCallback(async (clearMessage = true) => {
    try {
      const result = await api.getConfigPublic();
      if (result.ok) {
        setConfig(result.data);
        if (clearMessage) { setMessage(""); setMessageTone("info"); }
        return true;
      } else {
        setMessage(errorText(result.error));
        setMessageTone("error");
      }
    } catch {
      setMessage("IPC_UNAVAILABLE：无法读取本地配置");
      setMessageTone("error");
    }
    return false;
  }, []);
  useEffect(() => { void reload(); }, [reload]);

  async function run<T>(action: () => Promise<CommandResult<T>>, success: string) {
    setBusy(true);
    try {
      const result = await action();
      const refreshed = await reload(false);
      if (!result.ok) { announce(errorText(result.error), "error"); return result; }
      if (!refreshed) { announce("操作已完成，但无法重新读取配置。请重新打开服务页核对。", "error"); return result; }
      announce(success, "success");
      return result;
    } catch {
      await reload(false);
      announce("IPC_UNAVAILABLE：本地操作失败", "error");
      return null;
    } finally { setBusy(false); }
  }

  async function testProvider(id: string) {
    setBusy(true);
    announce("正在测试连接…", "pending");
    setProviderTests((current) => ({ ...current, [id]: { tone: "pending", text: "正在测试连接…" } }));
    try {
      const result = await api.testModelProvider(id);
      if (!result.ok) {
        const text = providerTestMessage(result.error);
        announce(text, "error");
        setProviderTests((current) => ({ ...current, [id]: { tone: "error", text } }));
        return;
      }
      if (!result.data.reachable) {
        const text = "连接测试失败：供应商不可达";
        announce(text, "error");
        setProviderTests((current) => ({ ...current, [id]: { tone: "error", text } }));
        return;
      }
      const text = providerTestSuccessMessage(result.data);
      const tone = result.data.webStatus === "authentication_failed" ? "error" : "success";
      announce(text, tone);
      setProviderTests((current) => ({ ...current, [id]: { tone, text } }));
    } catch {
      const text = "IPC_UNAVAILABLE：连接测试失败";
      announce(text, "error");
      setProviderTests((current) => ({ ...current, [id]: { tone: "error", text } }));
    } finally { setBusy(false); }
  }

  async function submitProvider(event: FormEvent) {
    event.preventDefault();
    try {
      const result = await run(() => api.saveModelProvider({ id: optional(provider.id), name: provider.name.trim() || null, baseUrl: provider.baseUrl.trim(), apiKey: optional(provider.apiKey), ...(provider.webCapability !== "none" ? { webCapability: provider.webCapability } : {}) }), "供应商已保存");
      if (result?.ok) {
        setProvider((current) => ({ ...current, id: result.data.id, apiKey: "" }));
        return;
      }
    } finally {
      setProvider((current) => ({ ...current, apiKey: "" }));
    }
  }

  async function discover(id: string) {
    setBusy(true);
    try {
      const result = await api.discoverModelProvider(id);
      if (result.ok) {
        setModels((current) => ({ ...current, [id]: result.data.models.map((model) => model.id) }));
        setMessage("已发现 " + result.data.models.length + " 个模型");
      } else setMessage(errorText(result.error));
    } catch {
      setMessage("IPC_UNAVAILABLE：模型发现失败");
    } finally { setBusy(false); }
  }

  // 线路表单里选中供应商即自动发现模型，模型字段的建议列表无需先去供应商页操作。
  // 静默进行：不占用全局 busy（避免打断填写/保存），失败时建议列表为空，模型 ID 仍可手动填写。
  // 已发现过的供应商不重复请求。
  function ensureDiscovered(id: string) {
    if (!id || models[id]) return;
    void (async () => {
      try {
        const result = await api.discoverModelProvider(id);
        if (result.ok) {
          setModels((current) => ({ ...current, [id]: result.data.models.map((model) => model.id) }));
        }
      } catch { /* 静默降级：建议列表不可用时用户仍可手填模型 ID */ }
    })();
  }

  async function submitRoute(event: FormEvent) {
    event.preventDefault();
    const cascaded = route.mode === "cascaded";
    const result = await run(() => api.saveSpeechRoute({
      id: optional(route.id), name: route.name.trim(), mode: route.mode,
      asrProviderId: cascaded ? optional(route.asrProviderId) : null,
      asrModelId: cascaded ? optional(route.asrModelId) : null,
      llmProviderId: cascaded ? optional(route.llmProviderId) : null,
      llmModelId: cascaded ? optional(route.llmModelId) : null,
      ttsProviderId: cascaded ? optional(route.ttsProviderId) : null,
      ttsModelId: cascaded ? optional(route.ttsModelId) : null,
      voiceId: optional(route.voiceId),
      e2eProviderId: cascaded ? null : optional(route.e2eProviderId),
      e2eModelId: cascaded ? null : optional(route.e2eModelId),
    }), "语音线路已保存，请先测试再启用");
    if (result?.ok) setRoute((current) => ({ ...current, id: result.data.id }));
  }

  const providers = config?.models.providers ?? [];
  const setRouteField = (key: keyof typeof route, value: string) => setRoute((current) => ({ ...current, [key]: value }));

  return <div className="services-page">
    <PageShell id="services" />
    <div className="configuration-layout">
      <nav className="settings-nav" aria-label="服务分类">
        {categories.map(({ id, label, icon: Icon }) => <button key={id} id={`services-category-${id}`} type="button" aria-current={category === id ? "page" : undefined} aria-controls={`services-panel-${id}`} onClick={() => setCategory(id)}><Icon size={16} aria-hidden="true" />{label}</button>)}
      </nav>
      <div className="configuration-content">
      {message && <p className="services-message" data-tone={messageTone} role="status" aria-live="polite">{message}</p>}
      {deletion && <section className="service-card" role="region" aria-label="删除供应商确认">
        <h3>{deletion.references.length ? "暂时无法删除" : deletion.cleanup ? "重试密钥清理" : "确认删除供应商"}：{deletion.name}</h3>
        {deletion.references.length ? <><p>请先修改或删除以下引用，不会自动删除关联配置。</p><ul>{deletion.references.map((reference) => <li key={`${reference.kind}/${reference.id}`}><button disabled={busy} onClick={() => openReference(reference)}>处理 {reference.kind === "voiceRoute" ? "语音线路" : "Embedding"}：{reference.name}</button></li>)}</ul></> : <><p>{deletion.cleanup ? "配置已删除，仅重试清理 Windows 凭据管理器中的密钥。" : "将删除此供应商配置和已保存的密钥，历史记录和资料文件不受影响。"}</p><button className="button-danger" disabled={busy} onClick={() => void confirmDeletion()}>{deletion.cleanup ? "重试清理密钥" : "确认删除供应商"}</button></>}
        <button disabled={busy} onClick={() => setDeletion(null)}>取消</button>
      </section>}
      <section id="services-panel-providers" className="service-panel" hidden={category !== "providers"} aria-labelledby="services-category-providers">
        <h2 className="section-heading">模型供应商</h2>
        <p className="configuration-description">连接 OpenAI 兼容服务。密钥仅保存到 Windows 凭据管理器。</p>
        <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submitProvider}>
          <h3>{provider.id && providers.some((item) => item.id === provider.id) ? "编辑供应商" : "添加供应商"}</h3>
          <label>显示名称<input required value={provider.name} onChange={(e) => setProvider({ ...provider, name: e.target.value })}/></label>
          <label>接入地址<input required type="url" placeholder="https://example.com/v1" value={provider.baseUrl} onChange={(e) => setProvider({ ...provider, baseUrl: e.target.value })}/></label>
          <label>API Key<input type="password" autoComplete="new-password" value={provider.apiKey} onChange={(e) => setProvider({ ...provider, apiKey: e.target.value })}/><small>留空会保留已保存的密钥</small></label>
          <label>联网搜索协议<select value={provider.webCapability} onChange={(e) => setProvider({ ...provider, webCapability: e.target.value as WebCapability })}>
            <option value="none">不启用 / 普通兼容接口</option>
            <option value="openai_responses_web_search">OpenAI Responses</option>
            <option value="qwen_responses_web_search">千问 Responses</option>
            <option value="qwen_chat_enable_search">千问 Chat Completions</option>
          </select><small>仅适用于支持原生搜索的模型，搜索可能另行计费。模型列表连接测试不代表联网搜索可用。</small></label>
          <button className="button-primary" disabled={busy} type="submit">保存供应商</button>
        </form>
        <div className="service-list configuration-list">
          <h3>已配置供应商 <span className="configuration-count">{providers.length}</span></h3>
          {providers.length === 0 && <p className="empty-state">还没有供应商。</p>}
          {providers.map((item) => <article className="service-card" key={item.id}>
            <h3>{item.name || "未命名供应商"}</h3><p>{item.baseUrl}</p>
            <p>密钥：{item.credential?.configured ? "已安全保存" : "未配置"}</p>
            {config?.models.activeProviderId === item.id && <span className="status-badge">当前默认</span>}
            {providerTests[item.id] && <p className="service-test-result" data-tone={providerTests[item.id].tone} role="status">{providerTests[item.id].text}</p>}
            <div className="service-actions">
              <button aria-label={"编辑 " + (item.name || "未命名供应商")} disabled={busy} onClick={() => setProvider({ id: item.id, name: item.name ?? "", baseUrl: item.baseUrl, apiKey: "", webCapability: item.webCapability ?? "none" })}>编辑</button>
              <button aria-label={"测试 " + (item.name || "未命名供应商")} disabled={busy} onClick={() => void testProvider(item.id)}>{providerTests[item.id]?.tone === "pending" ? "测试中…" : "测试"}</button>
              <button disabled={busy} onClick={() => void discover(item.id)}>发现模型</button>
              <button disabled={busy} onClick={() => void run(() => api.activateModelProvider(item.id), "默认供应商已更新")}>设为默认</button>
              <button className="button-danger" disabled={busy} onClick={() => void inspectDeletion(item.id, item.name ?? item.id)}>删除</button>
            </div>
            {models[item.id]?.length > 0 && <p>模型：{models[item.id].join("、")}</p>}
          </article>)}
        </div>
        </div>
      </section>
      <section id="services-panel-routes" className="service-panel" hidden={category !== "routes"} aria-labelledby="services-category-routes">
        <h2 className="section-heading">语音线路</h2>
        <p className="configuration-description">选择语音模型与音色，保存后先测试再启用。</p>
        <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submitRoute}>
          <h3>{route.id && config?.speech.voiceRoutes.some((item) => item.id === route.id) ? "编辑线路" : "添加线路"}</h3>
          <label>线路名称<input required value={route.name} onChange={(e) => setRouteField("name", e.target.value)}/></label>
          <label>模式<select value={route.mode} onChange={(e) => setRouteField("mode", e.target.value)}><option value="cascaded">级联 ASR → LLM → TTS</option><option value="e2e">端到端 Realtime</option></select></label>
          {route.mode === "cascaded" ? <>
            <ProviderModelFields prefix="ASR" providers={providers} provider={route.asrProviderId} model={route.asrModelId} modelChoices={models[route.asrProviderId] ?? []} onProvider={(v) => { setRouteField("asrProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("asrModelId", v)}/>
            <ProviderModelFields prefix="LLM" providers={providers} provider={route.llmProviderId} model={route.llmModelId} modelChoices={models[route.llmProviderId] ?? []} onProvider={(v) => { setRouteField("llmProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("llmModelId", v)}/>
            <ProviderModelFields prefix="TTS" providers={providers} provider={route.ttsProviderId} model={route.ttsModelId} modelChoices={models[route.ttsProviderId] ?? []} onProvider={(v) => { setRouteField("ttsProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("ttsModelId", v)}/>
          </> : <ProviderModelFields prefix="Realtime" providers={providers} provider={route.e2eProviderId} model={route.e2eModelId} modelChoices={models[route.e2eProviderId] ?? []} onProvider={(v) => { setRouteField("e2eProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("e2eModelId", v)}/>}
          <label>音色 ID（可选）<input list="cloned-voice-ids" value={route.voiceId} onChange={(e) => setRouteField("voiceId", e.target.value)}/></label>
          <datalist id="cloned-voice-ids">{voiceReferences.filter((item) => item.cloneStatus === "cloned" && item.voiceId).map((item) => <option key={item.id} value={item.voiceId ?? ""} label={item.name}/>)}</datalist>
          {providers.length === 0 && <p className="muted">请先在“模型供应商”中添加服务。</p>}
          <button className="button-primary" disabled={busy || providers.length === 0} type="submit">保存语音线路</button>
        </form>
        <div className="service-list configuration-list">
          <h3>已配置线路 <span className="configuration-count">{config?.speech.voiceRoutes.length ?? 0}</span></h3>
          {(config?.speech.voiceRoutes ?? []).length === 0 && <p className="empty-state">还没有语音线路。</p>}
          {config?.speech.voiceRoutes.map((item) => <article className="service-card" key={item.id}>
            <h3>{item.name}</h3><p>{item.mode === "cascaded" ? "级联" : "端到端"} · {item.ready ? "测试通过" : "尚未就绪"}</p>
            {item.active && <span className="status-badge">当前启用</span>}
            <div className="service-actions">
              <button aria-label={"编辑 " + item.name} disabled={busy} onClick={() => setRoute({
                id: item.id, name: item.name, mode: item.mode,
                asrProviderId: item.asrProviderId ?? "", asrModelId: item.asrModelId ?? "",
                llmProviderId: item.llmProviderId ?? "", llmModelId: item.llmModelId ?? "",
                ttsProviderId: item.ttsProviderId ?? "", ttsModelId: item.ttsModelId ?? "",
                voiceId: item.voiceId ?? "", e2eProviderId: item.e2eProviderId ?? "",
                e2eModelId: item.e2eModelId ?? "",
              })}>编辑</button>
              <button disabled={busy} onClick={() => void run(() => api.testSpeechRoute(item.id), "线路测试通过")}>测试</button>
              <button disabled={busy || !item.ready} onClick={() => void run(() => api.activateSpeechRoute(item.id), "语音线路已启用")}>启用</button>
              <button className="button-danger" disabled={busy} onClick={() => void run(() => api.deleteSpeechRoute(item.id), "语音线路已删除")}>删除</button>
            </div>
          </article>)}
        </div>
        </div>
      </section>
      <section id="services-panel-embedding" hidden={category !== "embedding"} aria-labelledby="services-category-embedding"><EmbeddingEditor focusId={embeddingFocusId} /></section>
      <section id="services-panel-voices" hidden={category !== "voices"} aria-labelledby="services-category-voices"><VoiceReferenceEditor visible={category === "voices"} /></section>
      </div>
    </div>
  </div>;
}

function ProviderModelFields(props: { prefix: string; providers: PublicConfig["models"]["providers"]; provider: string; model: string; modelChoices: string[]; onProvider: (value: string) => void; onModel: (value: string) => void }) {
  const listId = `models-${props.prefix.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
  return <div className="provider-model-fields">
    <label>{props.prefix} 供应商<select required value={props.provider} onChange={(e) => props.onProvider(e.target.value)}><option value="">请选择</option>{props.providers.map((item) => <option key={item.id} value={item.id}>{item.name || "未命名供应商"}</option>)}</select></label>
    <label>{props.prefix} 模型<input required list={listId} value={props.model} onChange={(e) => props.onModel(e.target.value)}/><datalist id={listId}>{props.modelChoices.map((model) => <option key={model} value={model}/>)}</datalist></label>
  </div>;
}
