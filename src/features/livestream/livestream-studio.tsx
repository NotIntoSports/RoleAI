import { useEffect, useState } from "react";
import { Camera, ChevronLeft, ChevronRight, CircleStop, Hand, MessageSquare, Pause, Play, RotateCcw } from "lucide-react";

import * as api from "../../api/commands";
import type { LivestreamMediaKind, LivestreamRuntime, MaterialSummary, ObsRuntimeStatus } from "../../generated/bindings";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import "../../styles/workspace.css";

const OUTPUT_STATE_KEYS: Record<string, DictionaryStringKey> = {
  idle: "livestream.outputState.idle",
  synthesizing: "livestream.outputState.synthesizing",
  playing: "livestream.outputState.playing",
  played: "livestream.outputState.played",
  cancelled: "livestream.outputState.cancelled",
  failed: "livestream.outputState.failed",
};

export function LivestreamStudio() {
  useT();
  const [materials, setMaterials] = useState<MaterialSummary[]>([]);
  const [selectedMaterials, setSelectedMaterials] = useState<string[]>([]);
  const [title, setTitle] = useState("");
  const [language, setLanguage] = useState("中文");
  const [maxSegments, setMaxSegments] = useState(6);
  const [loopEnabled, setLoopEnabled] = useState(false);
  const [mediaPath, setMediaPath] = useState("");
  const [mediaKind, setMediaKind] = useState<LivestreamMediaKind | "">("");
  const [runtime, setRuntime] = useState<LivestreamRuntime | null>(null);
  const [obs, setObs] = useState<ObsRuntimeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [question, setQuestion] = useState("");
  const [obsPassword, setObsPassword] = useState("");
  const [obsPasswordConfigured, setObsPasswordConfigured] = useState(false);

  useEffect(() => {
    void api.listMaterials().then((result) => { if (result.ok) setMaterials(result.data.filter((item) => item.status === "text_ready")); });
    void api.getLivestream().then((result) => { if (result.ok) setRuntime(result.data); });
    void api.getObsRuntimeStatus().then((result) => { if (result.ok) setObs(result.data); });
    void api.getObsPasswordStatus().then((result) => { if (result.ok) setObsPasswordConfigured(result.data.configured); });
  }, []);

  useEffect(() => {
    if (!runtime || !["playing", "paused"].includes(runtime.script.state)) return;
    const timer = window.setInterval(() => {
      void api.getLivestream().then((result) => { if (result.ok) setRuntime(result.data); });
    }, 500);
    return () => window.clearInterval(timer);
  }, [runtime?.script.state]);

  async function generate() {
    if (!title.trim() || selectedMaterials.length === 0) {
      setMessage(t("livestream.messages.needTitleAndMaterials"));
      return;
    }
    setBusy(true);
    setMessage(t("livestream.messages.generating"));
    try {
      const result = await api.generateLivestream({
        title: title.trim(),
        materialIds: selectedMaterials,
        language: language.trim() || "中文",
        maxSegments,
        loopEnabled,
        mediaPath: mediaPath.trim() || null,
        mediaKind: mediaKind || null,
      });
      if (result.ok) { setRuntime(result.data); setMessage(t("livestream.messages.generated")); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage(t("livestream.messages.ipcGenerateFailed")); }
    finally { setBusy(false); }
  }

  async function saveEdits() {
    if (!runtime) return;
    setBusy(true);
    try {
      const result = await api.createLivestreamDraft({
        title: runtime.script.title,
        segments: runtime.script.segments.map((segment) => ({
          title: segment.title,
          text: segment.text,
          estimatedSeconds: segment.estimatedSeconds,
          sources: segment.sources,
        })),
        loopEnabled: runtime.script.loopEnabled,
        mediaPath: mediaPath.trim() || null,
        mediaKind: mediaKind || null,
      });
      if (result.ok) { setRuntime(result.data); setMessage(t("livestream.messages.saved")); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } finally { setBusy(false); }
  }

  async function control(action: "confirm" | "start" | "pause" | "takeover" | "resume" | "previous" | "next" | "replay" | "complete") {
    setBusy(true);
    try {
      const result = await api.controlLivestream(action);
      if (result.ok) { setRuntime(result.data); setMessage(""); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage(t("livestream.messages.ipcControlFailed")); }
    finally { setBusy(false); }
  }

  async function askQuestion() {
    if (!question.trim()) return;
    setBusy(true);
    setMessage(t("livestream.messages.answering"));
    try {
      const result = await api.insertLivestreamQuestion(question.trim());
      if (result.ok) { setRuntime(result.data); setQuestion(""); setMessage(t("livestream.messages.questionInserted")); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage(t("livestream.messages.ipcQuestionFailed")); }
    finally { setBusy(false); }
  }

  async function obsControl(action: "start" | "stop") {
    setBusy(true);
    try {
      const result = action === "start" ? await api.startObsVirtualCamera() : await api.stopObsVirtualCamera();
      if (result.ok) { setObs(result.data); setMessage(result.data.errorCode ? t("livestream.messages.obsError", { code: result.data.errorCode }) : ""); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage(t("livestream.messages.obsStartFailed")); }
    finally { setBusy(false); }
  }

  async function saveObsCredential() {
    setBusy(true);
    try {
      const result = await api.saveObsPassword(obsPassword);
      if (result.ok) { setObsPasswordConfigured(result.data.configured); setObsPassword(""); setMessage(result.data.configured ? t("livestream.messages.obsPasswordSaved") : t("livestream.messages.obsPasswordCleared")); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage(t("livestream.messages.obsPasswordSaveFailed")); }
    finally { setBusy(false); }
  }

  function updateSegment(index: number, field: "title" | "text", value: string) {
    if (!runtime) return;
    setRuntime({
      ...runtime,
      script: {
        ...runtime.script,
        confirmed: false,
        state: "draft",
        segments: runtime.script.segments.map((segment, at) => at === index ? { ...segment, [field]: value, status: "draft" } : segment),
      },
    });
  }

  return <section className="workspace-session livestream-studio" aria-labelledby="livestream-heading">
    <header className="session-toolbar">
      <div><h2 id="livestream-heading">{t("livestream.heading")}</h2><p>{t("livestream.description")}</p></div>
      <div className="service-actions">
        <span className="status-badge" data-active={obs?.virtualCameraActive ?? false}>{obs?.virtualCameraActive ? t("livestream.cameraActive") : t("livestream.cameraInactive")}</span>
        <button disabled={busy || !runtime?.script.confirmed} onClick={() => void obsControl("start")}><Camera size={15} />{t("livestream.startObs")}</button>
        <button disabled={busy || !obs?.virtualCameraActive} onClick={() => void obsControl("stop")}><CircleStop size={15} />{t("livestream.stopObs")}</button>
      </div>
    </header>

    {message && <p role="status">{message}</p>}
    {runtime && <p role="status">{t("livestream.voiceRouteStage", { state: OUTPUT_STATE_KEYS[runtime.stage.outputState] ? t(OUTPUT_STATE_KEYS[runtime.stage.outputState]) : runtime.stage.outputState })}{runtime.stage.outputErrorCode ? `（${runtime.stage.outputErrorCode}）` : ""}</p>}
    <details className="livestream-configuration" open={!runtime}>
    <summary>{t("livestream.config.summary")}</summary>
    <section className="session-selection livestream-setup" aria-label={t("livestream.config.wizardAria")}>
      <label>{t("livestream.config.obsPasswordLabel")}<input type="password" value={obsPassword} onChange={(event) => setObsPassword(event.target.value)} placeholder={obsPasswordConfigured ? t("livestream.config.obsPasswordConfiguredPlaceholder") : t("livestream.config.obsPasswordPlaceholder")} /></label>
      <button disabled={busy} onClick={() => void saveObsCredential()}>{obsPasswordConfigured && !obsPassword ? t("livestream.config.clearObsPassword") : t("livestream.config.saveObsPassword")}</button>
      <label>{t("livestream.config.titleLabel")}<input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
      <label>{t("livestream.config.languageLabel")}<input value={language} onChange={(event) => setLanguage(event.target.value)} /></label>
      <label>{t("livestream.config.maxSegmentsLabel")}<input type="number" min={1} max={12} value={maxSegments} onChange={(event) => setMaxSegments(Number(event.target.value))} /></label>
      <label>{t("livestream.config.mediaPathLabel")}<input value={mediaPath} onChange={(event) => setMediaPath(event.target.value)} placeholder={t("livestream.config.mediaPathPlaceholder")} /></label>
      <label>{t("livestream.config.mediaKindLabel")}<select value={mediaKind} onChange={(event) => setMediaKind(event.target.value as LivestreamMediaKind | "")}><option value="">{t("livestream.config.mediaNone")}</option><option value="image">{t("livestream.config.mediaImage")}</option><option value="video">{t("livestream.config.mediaVideo")}</option></select></label>
      <label><input type="checkbox" checked={loopEnabled} onChange={(event) => setLoopEnabled(event.target.checked)} />{t("livestream.config.loopLabel")}</label>
      <fieldset><legend>{t("livestream.config.materialsLegend")}</legend>{materials.length === 0 ? <p>{t("livestream.config.noReadyMaterials")}</p> : materials.map((material) => <label key={material.id}><input type="checkbox" checked={selectedMaterials.includes(material.id)} onChange={(event) => setSelectedMaterials((current) => event.target.checked ? [...current, material.id] : current.filter((id) => id !== material.id))} />{material.fileName}</label>)}</fieldset>
      <button className="button-primary" disabled={busy} onClick={() => void generate()}>{t("livestream.config.generate")}</button>
    </section>
    </details>

    {runtime && <section className="livestream-script" aria-label={t("livestream.script.editorAria")}>
      <h3>{runtime.script.title}</h3>
      {runtime.script.segments.map((segment, index) => <article className="preflight-card" key={segment.id}>
        <label>{t("livestream.script.segmentTitleLabel")}<input value={segment.title} onChange={(event) => updateSegment(index, "title", event.target.value)} /></label>
        <label>{t("livestream.script.segmentTextLabel")}<textarea value={segment.text} onChange={(event) => updateSegment(index, "text", event.target.value)} /></label>
        <small>{t("livestream.script.estimatedSeconds", { n: segment.estimatedSeconds, sources: segment.sources.join("、") || t("livestream.script.localSources") })}</small>
      </article>)}
      <div className="service-actions">
        <button disabled={busy || runtime.script.confirmed} onClick={() => void saveEdits()}>{t("livestream.script.save")}</button>
        <button disabled={busy || runtime.script.confirmed} onClick={() => void control("confirm")}>{t("livestream.script.confirm")}</button>
        <button disabled={busy || !runtime.script.confirmed || runtime.script.state === "playing"} onClick={() => void control("start")}><Play size={15} />{t("livestream.script.start")}</button>
        <button disabled={busy || runtime.script.state !== "playing"} onClick={() => void control("pause")}><Pause size={15} />{t("livestream.script.pause")}</button>
        <button disabled={busy || runtime.script.state !== "playing"} onClick={() => void control("takeover")}><Hand size={15} />{t("livestream.script.takeover")}</button>
        <button disabled={busy || runtime.script.state !== "paused"} onClick={() => void control("resume")}><Play size={15} />{t("livestream.script.resume")}</button>
        <button disabled={busy || runtime.script.currentIndex === null} onClick={() => void control("previous")}><ChevronLeft size={15} />{t("livestream.script.previous")}</button>
        <button disabled={busy || runtime.script.currentIndex === null} onClick={() => void control("next")}><ChevronRight size={15} />{t("livestream.script.next")}</button>
        <button disabled={busy || runtime.script.currentIndex === null} onClick={() => void control("replay")}><RotateCcw size={15} />{t("livestream.script.replay")}</button>
      </div>
      <div className="service-actions">
        <label>{t("livestream.script.questionLabel")}<input value={question} onChange={(event) => setQuestion(event.target.value)} placeholder={t("livestream.script.questionPlaceholder")} /></label>
        <button disabled={busy || !runtime.script.confirmed || !question.trim()} onClick={() => void askQuestion()}><MessageSquare size={15} />{t("livestream.script.answerAndSpeak")}</button>
      </div>
    </section>}
    <p className="muted">{t("livestream.footnote")}</p>
  </section>;
}
