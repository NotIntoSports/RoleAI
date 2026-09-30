import { FormEvent, useCallback, useEffect, useState } from "react";
import { AudioLines, Boxes, Mic, Server } from "lucide-react";
import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { EmbeddingEditor } from "../../features/services/embedding-editor";
import { VoiceReferenceEditor } from "../../features/services/voice-reference-editor";
import type { CommandResult, ProviderDependency, ProviderTestResult, PublicConfig, VoiceReferenceSummary, VoiceRouteMode, WebCapability } from "../../generated/bindings";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import { PageShell } from "../page-shell";
import "../../styles/configuration.css";

const optional = (value: string) => value.trim() || null;
const errorKeys: Record<string, DictionaryStringKey> = {
  PROVIDER_IN_USE: "services.errors.PROVIDER_IN_USE",
  SECRET_CLEANUP_FAILED: "services.errors.SECRET_CLEANUP_FAILED",
  CONFIG_WRITE_FAILED: "services.errors.CONFIG_WRITE_FAILED",
};
const errorText = (error: { code: string; message: string; field?: string | null }) => {
  const key = errorKeys[error.code];
  return key ? t(key) : error.message;
};
const providerTestMessage = (error: { code: string; message: string; field?: string | null }) => {
  const keyed: Partial<Record<string, DictionaryStringKey>> = {
    PROVIDER_TIMEOUT: "services.providerTest.PROVIDER_TIMEOUT",
    PROVIDER_REQUEST_FAILED: "services.providerTest.PROVIDER_REQUEST_FAILED",
    PROVIDER_UNAUTHORIZED: "services.providerTest.PROVIDER_UNAUTHORIZED",
    PROVIDER_ENDPOINT_INVALID: "services.providerTest.PROVIDER_ENDPOINT_INVALID",
    PROVIDER_RESPONSE_INVALID: "services.providerTest.PROVIDER_RESPONSE_INVALID",
    PROVIDER_RESPONSE_TOO_LARGE: "services.providerTest.PROVIDER_RESPONSE_TOO_LARGE",
    PROVIDER_CLIENT_UNAVAILABLE: "services.providerTest.PROVIDER_CLIENT_UNAVAILABLE",
    PROVIDER_NOT_FOUND: "services.providerTest.PROVIDER_NOT_FOUND",
  };
  const key = keyed[error.code];
  return key ? t(key) : errorText(error);
};
const providerTestSuccessMessage = (result: ProviderTestResult) => {
  const connection = t("services.providerTest.successBase", { n: result.modelCount });
  switch (result.webStatus) {
    case "available": return t("services.providerTest.webAvailable", { connection, n: result.webSourceCount });
    case "model_unsupported": return t("services.providerTest.webModelUnsupported", { connection });
    case "interface_incompatible": return t("services.providerTest.webInterfaceIncompatible", { connection });
    case "network_unreachable": return t("services.providerTest.webNetworkUnreachable", { connection });
    case "authentication_failed": return t("services.providerTest.webAuthFailed");
    case "disabled": return t("services.providerTest.webDisabled", { connection });
  }
};
const initialRoute = { id: "", name: "", mode: "cascaded" as VoiceRouteMode, asrProviderId: "", asrModelId: "", llmProviderId: "", llmModelId: "", ttsProviderId: "", ttsModelId: "", voiceId: "", e2eProviderId: "", e2eModelId: "" };
type MessageTone = "info" | "pending" | "success" | "error";
type ProviderTestState = { tone: Exclude<MessageTone, "info">; text: string };
const categories = [
  { id: "providers", labelKey: "services.categories.providers", icon: Server },
  { id: "routes", labelKey: "services.categories.routes", icon: AudioLines },
  { id: "voices", labelKey: "services.categories.voices", icon: Mic },
  { id: "embedding", labelKey: "services.categories.embedding", icon: Boxes },
] as const;

export function ServicesPage() {
  useT();
  const [category, setCategory] = useState<(typeof categories)[number]["id"]>(() => {
    const requested = new URLSearchParams(window.location.search).get("category");
    return categories.find((item) => item.id === requested)?.id ?? "providers";
  });
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [message, setMessage] = useState(() => t("services.reading"));
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
    } catch { announce(t("services.cannotInspectReferences"), "error"); }
    finally { setBusy(false); }
  }

  async function confirmDeletion() {
    if (!deletion) return;
    const target = deletion;
    const result = await run(() => api.deleteModelProvider(target.id), t("services.providers.deleted"));
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
    announce(t("services.deletion.referenceNotice", { name: reference.name, id: reference.id }), "info");
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
      setMessage(t("services.ipc.statusUnavailable"));
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
      if (!refreshed) { announce(t("services.refreshedButStale"), "error"); return result; }
      announce(success, "success");
      return result;
    } catch {
      await reload(false);
      announce(t("services.ipc.operateFailed"), "error");
      return null;
    } finally { setBusy(false); }
  }

  async function testProvider(id: string) {
    setBusy(true);
    announce(t("services.providerTest.testing"), "pending");
    setProviderTests((current) => ({ ...current, [id]: { tone: "pending", text: t("services.providerTest.testing") } }));
    try {
      const result = await api.testModelProvider(id);
      if (!result.ok) {
        const text = providerTestMessage(result.error);
        announce(text, "error");
        setProviderTests((current) => ({ ...current, [id]: { tone: "error", text } }));
        return;
      }
      if (!result.data.reachable) {
        const text = t("services.providerTest.unreachable");
        announce(text, "error");
        setProviderTests((current) => ({ ...current, [id]: { tone: "error", text } }));
        return;
      }
      const text = providerTestSuccessMessage(result.data);
      const tone = result.data.webStatus === "authentication_failed" ? "error" : "success";
      announce(text, tone);
      setProviderTests((current) => ({ ...current, [id]: { tone, text } }));
    } catch {
      const text = t("services.ipc.testFailed");
      announce(text, "error");
      setProviderTests((current) => ({ ...current, [id]: { tone: "error", text } }));
    } finally { setBusy(false); }
  }

  async function submitProvider(event: FormEvent) {
    event.preventDefault();
    try {
      const result = await run(() => api.saveModelProvider({ id: optional(provider.id), name: provider.name.trim() || null, baseUrl: provider.baseUrl.trim(), apiKey: optional(provider.apiKey), ...(provider.webCapability !== "none" ? { webCapability: provider.webCapability } : {}) }), t("services.providers.saved"));
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
        setMessage(t("services.discoveredModels", { n: result.data.models.length }));
      } else setMessage(errorText(result.error));
    } catch {
      setMessage(t("services.ipc.discoverFailed"));
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
    }), t("services.routes.saved"));
    if (result?.ok) setRoute((current) => ({ ...current, id: result.data.id }));
  }

  const providers = config?.models.providers ?? [];
  const setRouteField = (key: keyof typeof route, value: string) => setRoute((current) => ({ ...current, [key]: value }));

  return <div className="services-page">
    <PageShell id="services" />
    <div className="configuration-layout">
      <nav className="settings-nav" aria-label={t("services.navAria")}>
        {categories.map(({ id, labelKey, icon: Icon }) => <button key={id} id={`services-category-${id}`} type="button" aria-current={category === id ? "page" : undefined} aria-controls={`services-panel-${id}`} onClick={() => setCategory(id)}><Icon size={16} aria-hidden="true" />{t(labelKey)}</button>)}
      </nav>
      <div className="configuration-content">
      {message && <p className="services-message" data-tone={messageTone} role="status" aria-live="polite">{message}</p>}
      {deletion && <section className="service-card" role="region" aria-label={t("services.deletion.regionAria")}>
        <h3>{deletion.references.length ? t("services.deletion.blockedHeading", { name: deletion.name }) : deletion.cleanup ? t("services.deletion.cleanupHeading", { name: deletion.name }) : t("services.deletion.confirmHeading", { name: deletion.name })}</h3>
        {deletion.references.length ? <><p>{t("services.deletion.blockedHint")}</p><ul>{deletion.references.map((reference) => <li key={`${reference.kind}/${reference.id}`}><button disabled={busy} onClick={() => openReference(reference)}>{t("services.deletion.referenceAction", { kind: reference.kind === "voiceRoute" ? t("services.deletion.kindVoiceRoute") : t("services.deletion.kindEmbedding"), name: reference.name })}</button></li>)}</ul></> : <><p>{deletion.cleanup ? t("services.deletion.cleanupHint") : t("services.deletion.confirmHint")}</p><button className="button-danger" disabled={busy} onClick={() => void confirmDeletion()}>{deletion.cleanup ? t("services.deletion.retryCleanup") : t("services.deletion.confirmAction")}</button></>}
        <button disabled={busy} onClick={() => setDeletion(null)}>{t("services.deletion.cancel")}</button>
      </section>}
      <section id="services-panel-providers" className="service-panel" hidden={category !== "providers"} aria-labelledby="services-category-providers">
        <h2 className="section-heading">{t("services.providers.heading")}</h2>
        <p className="configuration-description">{t("services.providers.description")}</p>
        <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submitProvider}>
          <h3>{provider.id && providers.some((item) => item.id === provider.id) ? t("services.providers.editTitle") : t("services.providers.addTitle")}</h3>
          <label>{t("services.providers.nameLabel")}<input required value={provider.name} onChange={(e) => setProvider({ ...provider, name: e.target.value })}/></label>
          <label>{t("services.providers.endpointLabel")}<input required type="url" placeholder="https://example.com/v1" value={provider.baseUrl} onChange={(e) => setProvider({ ...provider, baseUrl: e.target.value })}/></label>
          <label>API Key<input type="password" autoComplete="new-password" value={provider.apiKey} onChange={(e) => setProvider({ ...provider, apiKey: e.target.value })}/><small>{t("services.providers.apiKeyHint")}</small></label>
          <label>{t("services.providers.webSearchLabel")}<select value={provider.webCapability} onChange={(e) => setProvider({ ...provider, webCapability: e.target.value as WebCapability })}>
            <option value="none">{t("services.providers.webSearchNone")}</option>
            <option value="openai_responses_web_search">OpenAI Responses</option>
            <option value="qwen_responses_web_search">{t("services.providers.webSearchQwenResponses")}</option>
            <option value="qwen_chat_enable_search">{t("services.providers.webSearchQwenChat")}</option>
          </select><small>{t("services.providers.webSearchHint")}</small></label>
          <button className="button-primary" disabled={busy} type="submit">{t("services.providers.save")}</button>
        </form>
        <div className="service-list configuration-list">
          <h3>{t("services.providers.listHeading")} <span className="configuration-count">{providers.length}</span></h3>
          {providers.length === 0 && <EmptyState title={t("services.providers.empty")} />}
          {providers.map((item) => <article className="service-card" key={item.id}>
            <h3>{item.name || t("services.providers.unnamed")}</h3><p>{item.baseUrl}</p>
            <p>{t("services.providers.credentialLine", { state: item.credential?.configured ? t("services.providers.credentialSaved") : t("services.providers.credentialMissing") })}</p>
            {config?.models.activeProviderId === item.id && <span className="status-badge">{t("services.providers.currentDefault")}</span>}
            {providerTests[item.id] && <p className="service-test-result" data-tone={providerTests[item.id].tone} role="status">{providerTests[item.id].text}</p>}
            <div className="service-actions">
              <button aria-label={t("services.providers.editAria", { name: item.name || t("services.providers.unnamed") })} disabled={busy} onClick={() => setProvider({ id: item.id, name: item.name ?? "", baseUrl: item.baseUrl, apiKey: "", webCapability: item.webCapability ?? "none" })}>{t("services.providers.edit")}</button>
              <button aria-label={t("services.providers.testAria", { name: item.name || t("services.providers.unnamed") })} disabled={busy} onClick={() => void testProvider(item.id)}>{providerTests[item.id]?.tone === "pending" ? t("services.providers.testing") : t("services.providers.test")}</button>
              <button disabled={busy} onClick={() => void discover(item.id)}>{t("services.providers.discover")}</button>
              <button disabled={busy} onClick={() => void run(() => api.activateModelProvider(item.id), t("services.providers.defaultUpdated"))}>{t("services.providers.makeDefault")}</button>
              <button className="button-danger" disabled={busy} onClick={() => void inspectDeletion(item.id, item.name ?? item.id)}>{t("services.providers.delete")}</button>
            </div>
            {models[item.id]?.length > 0 && <p>{t("services.providers.modelsLine", { models: models[item.id].join("、") })}</p>}
          </article>)}
        </div>
        </div>
      </section>
      <section id="services-panel-routes" className="service-panel" hidden={category !== "routes"} aria-labelledby="services-category-routes">
        <h2 className="section-heading">{t("services.routes.heading")}</h2>
        <p className="configuration-description">{t("services.routes.description")}</p>
        <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submitRoute}>
          <h3>{route.id && config?.speech.voiceRoutes.some((item) => item.id === route.id) ? t("services.routes.editTitle") : t("services.routes.addTitle")}</h3>
          <label>{t("services.routes.nameLabel")}<input required value={route.name} onChange={(e) => setRouteField("name", e.target.value)}/></label>
          <label>{t("services.routes.modeLabel")}<select value={route.mode} onChange={(e) => setRouteField("mode", e.target.value)}><option value="cascaded">{t("services.routes.modeCascaded")}</option><option value="e2e">{t("services.routes.modeE2e")}</option></select></label>
          {route.mode === "cascaded" ? <>
            <ProviderModelFields prefix="ASR" providers={providers} provider={route.asrProviderId} model={route.asrModelId} modelChoices={models[route.asrProviderId] ?? []} onProvider={(v) => { setRouteField("asrProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("asrModelId", v)}/>
            <ProviderModelFields prefix="LLM" providers={providers} provider={route.llmProviderId} model={route.llmModelId} modelChoices={models[route.llmProviderId] ?? []} onProvider={(v) => { setRouteField("llmProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("llmModelId", v)}/>
            <ProviderModelFields prefix="TTS" providers={providers} provider={route.ttsProviderId} model={route.ttsModelId} modelChoices={models[route.ttsProviderId] ?? []} onProvider={(v) => { setRouteField("ttsProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("ttsModelId", v)}/>
          </> : <ProviderModelFields prefix="Realtime" providers={providers} provider={route.e2eProviderId} model={route.e2eModelId} modelChoices={models[route.e2eProviderId] ?? []} onProvider={(v) => { setRouteField("e2eProviderId", v); ensureDiscovered(v); }} onModel={(v) => setRouteField("e2eModelId", v)}/>}
          <label>{t("services.routes.voiceIdLabel")}<input list="cloned-voice-ids" value={route.voiceId} onChange={(e) => setRouteField("voiceId", e.target.value)}/></label>
          <datalist id="cloned-voice-ids">{voiceReferences.filter((item) => item.cloneStatus === "cloned" && item.voiceId).map((item) => <option key={item.id} value={item.voiceId ?? ""} label={item.name}/>)}</datalist>
          {providers.length === 0 && <p className="muted">{t("services.routes.noProvidersHint")}</p>}
          <button className="button-primary" disabled={busy || providers.length === 0} type="submit">{t("services.routes.save")}</button>
        </form>
        <div className="service-list configuration-list">
          <h3>{t("services.routes.listHeading")} <span className="configuration-count">{config?.speech.voiceRoutes.length ?? 0}</span></h3>
          {(config?.speech.voiceRoutes ?? []).length === 0 && <EmptyState title={t("services.routes.empty")} />}
          {config?.speech.voiceRoutes.map((item) => <article className="service-card" key={item.id}>
            <h3>{item.name}</h3><p>{item.mode === "cascaded" ? t("services.routes.cascadedShort") : t("services.routes.e2eShort")} · {item.ready ? t("services.routes.ready") : t("services.routes.notReady")}</p>
            {item.active && <span className="status-badge">{t("services.routes.activeBadge")}</span>}
            <div className="service-actions">
              <button aria-label={t("services.routes.editAria", { name: item.name })} disabled={busy} onClick={() => setRoute({
                id: item.id, name: item.name, mode: item.mode,
                asrProviderId: item.asrProviderId ?? "", asrModelId: item.asrModelId ?? "",
                llmProviderId: item.llmProviderId ?? "", llmModelId: item.llmModelId ?? "",
                ttsProviderId: item.ttsProviderId ?? "", ttsModelId: item.ttsModelId ?? "",
                voiceId: item.voiceId ?? "", e2eProviderId: item.e2eProviderId ?? "",
                e2eModelId: item.e2eModelId ?? "",
              })}>{t("services.routes.edit")}</button>
              <button disabled={busy} onClick={() => void run(() => api.testSpeechRoute(item.id), t("services.routes.testPassed"))}>{t("services.routes.test")}</button>
              <button disabled={busy || !item.ready} onClick={() => void run(() => api.activateSpeechRoute(item.id), t("services.routes.activated"))}>{t("services.routes.activate")}</button>
              <button className="button-danger" disabled={busy} onClick={() => void run(() => api.deleteSpeechRoute(item.id), t("services.routes.deleted"))}>{t("services.routes.delete")}</button>
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
    <label>{t("services.routes.providerLabel", { prefix: props.prefix })}<select required value={props.provider} onChange={(e) => props.onProvider(e.target.value)}><option value="">{t("services.routes.pickProvider")}</option>{props.providers.map((item) => <option key={item.id} value={item.id}>{item.name || t("services.providers.unnamed")}</option>)}</select></label>
    <label>{t("services.routes.modelLabel", { prefix: props.prefix })}<input required list={listId} value={props.model} onChange={(e) => props.onModel(e.target.value)}/><datalist id={listId}>{props.modelChoices.map((model) => <option key={model} value={model}/>)}</datalist></label>
  </div>;
}
