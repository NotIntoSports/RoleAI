import { useEffect, useState } from "react";
import { Camera, ChevronLeft, ChevronRight, CircleStop, Hand, MessageSquare, Pause, Play, RotateCcw } from "lucide-react";

import * as api from "../../api/commands";
import type { LivestreamMediaKind, LivestreamRuntime, MaterialSummary, ObsRuntimeStatus } from "../../generated/bindings";
import "../../styles/workspace.css";

export function LivestreamStudio() {
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
      setMessage("请输入产品标题并至少选择一份已就绪资料。");
      return;
    }
    setBusy(true);
    setMessage("正在根据本地资料生成有限分段讲稿…");
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
      if (result.ok) { setRuntime(result.data); setMessage("讲稿已生成，请编辑并确认后播放。"); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage("IPC_UNAVAILABLE：讲稿生成失败"); }
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
      if (result.ok) { setRuntime(result.data); setMessage("修改已保存，请确认讲稿。"); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } finally { setBusy(false); }
  }

  async function control(action: "confirm" | "start" | "pause" | "takeover" | "resume" | "previous" | "next" | "replay" | "complete") {
    setBusy(true);
    try {
      const result = await api.controlLivestream(action);
      if (result.ok) { setRuntime(result.data); setMessage(""); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage("IPC_UNAVAILABLE：直播控制失败"); }
    finally { setBusy(false); }
  }

  async function askQuestion() {
    if (!question.trim()) return;
    setBusy(true);
    setMessage("正在根据已确认讲稿回答人工问题…");
    try {
      const result = await api.insertLivestreamQuestion(question.trim());
      if (result.ok) { setRuntime(result.data); setQuestion(""); setMessage("人工问题已插入，回答完成后可继续原讲稿。"); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage("IPC_UNAVAILABLE：人工问题处理失败"); }
    finally { setBusy(false); }
  }

  async function obsControl(action: "start" | "stop") {
    setBusy(true);
    try {
      const result = action === "start" ? await api.startObsVirtualCamera() : await api.stopObsVirtualCamera();
      if (result.ok) { setObs(result.data); setMessage(result.data.errorCode ? `OBS：${result.data.errorCode}` : ""); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage("OBS_CONTROL_FAILED：无法连接本机 OBS"); }
    finally { setBusy(false); }
  }

  async function saveObsCredential() {
    setBusy(true);
    try {
      const result = await api.saveObsPassword(obsPassword);
      if (result.ok) { setObsPasswordConfigured(result.data.configured); setObsPassword(""); setMessage(result.data.configured ? "OBS 密码已安全保存。" : "OBS 密码已清除。"); }
      else setMessage(`${result.error.code}：${result.error.message}`);
    } catch { setMessage("SECRET_BACKEND_UNAVAILABLE：OBS 密码保存失败"); }
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
      <div><h2 id="livestream-heading">虚拟直播</h2><p>使用本地产品资料生成可控讲稿，图片或循环视频通过 OBS Virtual Camera 输出。</p></div>
      <div className="service-actions">
        <span className="status-badge" data-active={obs?.virtualCameraActive ?? false}>{obs?.virtualCameraActive ? "虚拟摄像头已启动" : "虚拟摄像头未启动"}</span>
        <button disabled={busy || !runtime?.script.confirmed} onClick={() => void obsControl("start")}><Camera size={15} />启动 OBS 输出</button>
        <button disabled={busy || !obs?.virtualCameraActive} onClick={() => void obsControl("stop")}><CircleStop size={15} />停止 OBS 输出</button>
      </div>
    </header>

    {message && <p role="status">{message}</p>}
    {runtime && <p role="status">语音线路：{{ idle: "待机", synthesizing: "正在合成", playing: "正在实际播放", played: "已实际播放", cancelled: "已取消/人工接管", failed: "播放失败" }[runtime.stage.outputState]}{runtime.stage.outputErrorCode ? `（${runtime.stage.outputErrorCode}）` : ""}</p>}
    <details className="livestream-configuration" open={!runtime}>
    <summary>直播配置与产品资料</summary>
    <section className="session-selection livestream-setup" aria-label="直播向导">
      <label>OBS WebSocket 密码<input type="password" value={obsPassword} onChange={(event) => setObsPassword(event.target.value)} placeholder={obsPasswordConfigured ? "已配置；留空并保存可清除" : "仅保存到 Windows 凭据管理器"} /></label>
      <button disabled={busy} onClick={() => void saveObsCredential()}>{obsPasswordConfigured && !obsPassword ? "清除 OBS 密码" : "保存 OBS 密码"}</button>
      <label>产品标题<input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
      <label>讲解语言<input value={language} onChange={(event) => setLanguage(event.target.value)} /></label>
      <label>最多段数<input type="number" min={1} max={12} value={maxSegments} onChange={(event) => setMaxSegments(Number(event.target.value))} /></label>
      <label>人物图片或循环视频路径<input value={mediaPath} onChange={(event) => setMediaPath(event.target.value)} placeholder="可选：本机图片或视频文件" /></label>
      <label>素材类型<select value={mediaKind} onChange={(event) => setMediaKind(event.target.value as LivestreamMediaKind | "")}><option value="">无素材</option><option value="image">图片</option><option value="video">循环视频</option></select></label>
      <label><input type="checkbox" checked={loopEnabled} onChange={(event) => setLoopEnabled(event.target.checked)} />讲稿播完后循环</label>
      <fieldset><legend>产品资料</legend>{materials.length === 0 ? <p>暂无已就绪资料，请先到“资料”页导入并建立索引。</p> : materials.map((material) => <label key={material.id}><input type="checkbox" checked={selectedMaterials.includes(material.id)} onChange={(event) => setSelectedMaterials((current) => event.target.checked ? [...current, material.id] : current.filter((id) => id !== material.id))} />{material.fileName}</label>)}</fieldset>
      <button className="button-primary" disabled={busy} onClick={() => void generate()}>生成分段讲稿</button>
    </section>
    </details>

    {runtime && <section className="livestream-script" aria-label="讲稿编辑">
      <h3>{runtime.script.title}</h3>
      {runtime.script.segments.map((segment, index) => <article className="preflight-card" key={segment.id}>
        <label>段落标题<input value={segment.title} onChange={(event) => updateSegment(index, "title", event.target.value)} /></label>
        <label>讲稿内容<textarea value={segment.text} onChange={(event) => updateSegment(index, "text", event.target.value)} /></label>
        <small>预计 {segment.estimatedSeconds} 秒 · 来源：{segment.sources.join("、") || "本地资料"}</small>
      </article>)}
      <div className="service-actions">
        <button disabled={busy || runtime.script.confirmed} onClick={() => void saveEdits()}>保存修改</button>
        <button disabled={busy || runtime.script.confirmed} onClick={() => void control("confirm")}>确认讲稿</button>
        <button disabled={busy || !runtime.script.confirmed || runtime.script.state === "playing"} onClick={() => void control("start")}><Play size={15} />开始播报</button>
        <button disabled={busy || runtime.script.state !== "playing"} onClick={() => void control("pause")}><Pause size={15} />暂停</button>
        <button disabled={busy || runtime.script.state !== "playing"} onClick={() => void control("takeover")}><Hand size={15} />人工接管</button>
        <button disabled={busy || runtime.script.state !== "paused"} onClick={() => void control("resume")}><Play size={15} />继续</button>
        <button disabled={busy || runtime.script.currentIndex === null} onClick={() => void control("previous")}><ChevronLeft size={15} />上一段</button>
        <button disabled={busy || runtime.script.currentIndex === null} onClick={() => void control("next")}><ChevronRight size={15} />下一段</button>
        <button disabled={busy || runtime.script.currentIndex === null} onClick={() => void control("replay")}><RotateCcw size={15} />重讲</button>
      </div>
      <div className="service-actions">
        <label>插入人工问题<input value={question} onChange={(event) => setQuestion(event.target.value)} placeholder="例如：这款产品适合什么场景？" /></label>
        <button disabled={busy || !runtime.script.confirmed || !question.trim()} onClick={() => void askQuestion()}><MessageSquare size={15} />回答并播报</button>
      </div>
    </section>}
    <p className="muted">首期不读取弹幕，不配置平台账号，不自动持续生成内容。Virtual Camera 只传视频，声音使用独立虚拟音频线路。</p>
  </section>;
}
