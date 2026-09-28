import { FormEvent, useCallback, useEffect, useRef, useState } from "react";

import * as api from "../../api/commands";
import type { CommandResult, PublicConfig, VoiceReferenceSummary } from "../../generated/bindings";
import { RECORD_MAX_MS, RECORD_MIN_MS, VoiceRecorder, bytesToBase64, type RecordingResult } from "./wav-recorder";

const errorText = (error: { code: string; message: string; field?: string | null }) =>
  `${error.field ? error.field + "：" : ""}${error.code}：${error.message}`;

const emptyReference = { id: "", name: "", providerId: "", targetModel: "" };

const optional = (value: string) => value.trim() || null;

const statusText: Record<string, string> = {
  pending: "未克隆",
  uploaded: "已上传，等待克隆结果",
  cloned: "已克隆",
  failed: "克隆失败",
};

export function VoiceReferenceEditor({ visible }: { visible: boolean }) {
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [references, setReferences] = useState<VoiceReferenceSummary[]>([]);
  const [message, setMessage] = useState("正在读取音色…");
  const [busy, setBusy] = useState(false);
  const [reference, setReference] = useState(emptyReference);
  const [recording, setRecording] = useState(false);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [audio, setAudio] = useState<{ base64: string; label: string } | null>(null);
  const recorderRef = useRef<VoiceRecorder | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const reload = useCallback(async () => {
    const problems: string[] = [];
    try {
      // 配置与列表分开兜底：任一失败不影响另一侧（供应商下拉不能因列表失败而空白）。
      const configResult = await api.getConfigPublic();
      if (configResult.ok) {
        setConfig(configResult.data);
      } else {
        problems.push(errorText(configResult.error));
      }
      const listResult = await api.listVoiceReferences();
      if (listResult.ok) {
        setReferences(listResult.data);
      } else {
        problems.push(errorText(listResult.error));
      }
      if (problems.length > 0) {
        setMessage(problems.join("；"));
      } else {
        setMessage("");
      }
    } catch (error) {
      setMessage(`IPC_UNAVAILABLE：无法读取音色（${String(error)}）`);
    }
  }, []);

  useEffect(() => {
    // 面板常驻挂载（服务页切标签只切显隐），供应商可能在其他标签新增；
    // 每次面板变为可见时重拉配置，避免供应商下拉停留在页面加载时的快照。
    if (visible) void reload();
  }, [visible, reload]);

  async function run<T>(action: () => Promise<CommandResult<T>>, success: string): Promise<CommandResult<T> | null> {
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
    } catch (error) {
      await reload();
      setMessage(`IPC_UNAVAILABLE：本地操作失败（${error instanceof Error ? error.message : String(error)}）`);
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (audio) {
      const result = await run(
        () =>
          api.saveVoiceReferenceAudio({
            id: optional(reference.id),
            name: reference.name.trim(),
            providerId: optional(reference.providerId),
            targetModel: optional(reference.targetModel),
            transcript: null,
            audioBase64: audio.base64,
          }),
        reference.id
          ? "音色与音频已更新；克隆状态已重置，请重新克隆"
          : "音色已保存，请点击「克隆」生成音色 ID",
      );
      if (result?.ok) {
        setReference((current) => ({ ...current, id: result.data.id }));
        clearAudio();
      }
      return;
    }
    if (reference.id) {
      await run(
        () =>
          api.updateVoiceReference({
            id: reference.id,
            name: reference.name.trim(),
            providerId: optional(reference.providerId),
            targetModel: optional(reference.targetModel),
            transcript: null,
          }),
        "音色信息已更新，原音频与克隆状态保持不变",
      );
      return;
    }
    setMessage("请先录制或选择参考音频。");
  }

  const startRecording = useCallback(async () => {
    const recorder = new VoiceRecorder();
    try {
      await recorder.start();
    } catch (error) {
      setMessage(`无法开始录音（${error instanceof DOMException ? error.name : String(error)}）：请检查麦克风权限与输入设备后重试。`);
      return;
    }
    recorderRef.current = recorder;
    setElapsedMs(0);
    setAudio(null);
    setRecording(true);
    setMessage("");
  }, []);

  const finishRecording = useCallback(async () => {
    const recorder = recorderRef.current;
    if (!recorder) return;
    recorderRef.current = null;
    setRecording(false);
    const result: RecordingResult | null = await recorder.stop();
    if (!result || result.durationMs < RECORD_MIN_MS) {
      setMessage(
        result
          ? `录音太短（仅 ${Math.round(result.durationMs / 1000)} 秒），请至少录制 3 秒后重试。`
          : "没有录到声音（未捕获到音频数据），请重试；若再次出现，请检查系统麦克风输入设备。",
      );
      return;
    }
    setAudio({ base64: result.base64, label: `已录制 ${Math.round(result.durationMs / 1000)} 秒 WAV` });
    setMessage("");
  }, []);

  const clearAudio = useCallback(() => {
    recorderRef.current?.cancel();
    recorderRef.current = null;
    setRecording(false);
    setAudio(null);
    setElapsedMs(0);
    if (fileInputRef.current) fileInputRef.current.value = "";
  }, []);

  const onFilePicked = useCallback(async (file: File | undefined) => {
    if (!file) return;
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      setAudio({ base64: bytesToBase64(bytes), label: `已选择 ${file.name}` });
      setMessage("");
    } catch {
      setMessage("无法读取所选音频文件，请重试。");
    }
  }, []);

  useEffect(() => {
    if (!recording) return;
    const timer = window.setInterval(() => {
      const recorder = recorderRef.current;
      if (!recorder) return;
      const ms = recorder.recordedMs;
      setElapsedMs(ms);
      if (ms >= RECORD_MAX_MS) void finishRecording();
    }, 250);
    return () => window.clearInterval(timer);
  }, [recording, finishRecording]);

  useEffect(() => clearAudio, [clearAudio]);

  const providers = config?.models.providers ?? [];

  return (
    <section className="service-panel voice-reference-editor" aria-labelledby="voice-reference-editor-heading">
      <h2 className="section-heading" id="voice-reference-editor-heading">音色克隆</h2>
      <p className="configuration-description">
        保存 3–30 秒（建议 10–15 秒）的参考音频，克隆后把返回的音色 ID 填入语音线路。音频保存在本机；点击「克隆」会上传到所选供应商并可能产生费用。
      </p>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{reference.id && references.some((item) => item.id === reference.id) ? "编辑音色" : "添加音色"}</h3>
          <label>
            名称
            <input
              required
              value={reference.name}
              onChange={(event) => setReference({ ...reference, name: event.target.value })}
            />
          </label>
          <label>
            供应商
            <select
              value={reference.providerId}
              onChange={(event) => setReference({ ...reference, providerId: event.target.value })}
            >
              <option value="">暂不选择（克隆前需指定）</option>
              {providers.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name || "未命名供应商"}
                </option>
              ))}
            </select>
          </label>
          <label>
            目标模型（可选）
            <input
              value={reference.targetModel}
              onChange={(event) => setReference({ ...reference, targetModel: event.target.value })}
            />
            <small>
              阿里云端到端 Realtime 系（如 qwen3.8-omni-flash-realtime）必填同名模型；
              留空按 CosyVoice（cosyvoice-v2）复刻，智谱无需填写。
            </small>
          </label>
          <div className="voice-recorder-field">
            <span>参考音频</span>
            <span className="voice-recorder">
              {recording ? (
                <button type="button" disabled={busy} onClick={() => void finishRecording()}>
                  停止录音（{Math.floor(elapsedMs / 1000)} 秒 / 30 秒上限）
                </button>
              ) : (
                <button type="button" disabled={busy} onClick={() => void startRecording()}>
                  开始录音
                </button>
              )}
              <button type="button" disabled={busy} onClick={() => fileInputRef.current?.click()}>
                选择音频文件…
              </button>
              <input
                ref={fileInputRef}
                type="file"
                accept=".wav,.mp3,audio/wav,audio/mpeg"
                hidden
                onChange={(event) => void onFilePicked(event.target.files?.[0])}
              />
            </span>
            {audio && (
              <span className="voice-recorder-status">
                {audio.label}
                <button type="button" disabled={busy} onClick={clearAudio}>
                  清除
                </button>
              </span>
            )}
            {!audio && reference.id && (
              <span className="voice-recorder-status">保存修改将保留原有音频与克隆状态</span>
            )}
            <small>录制或选择 wav / mp3（10 MB 以内，建议 10–15 秒）；首次录音需在系统提示中允许麦克风权限</small>
          </div>
          <button
            className="button-primary"
            disabled={busy}
            title={!audio && !reference.id ? "请先录制或选择参考音频" : undefined}
            type="submit"
          >
            {audio || !reference.id ? "保存音色" : "保存修改"}
          </button>
        </form>
        <div className="service-list configuration-list">
          <h3>已保存音色 <span className="configuration-count">{references.length}</span></h3>
          {references.length === 0 && <p className="empty-state">还没有音色。</p>}
          {references.map((item) => {
            const providerName = providers.find((provider) => provider.id === item.providerId)?.name;
            return (
              <article className="service-card" key={item.id}>
                <h3>{item.name}</h3>
                <p>
                  {providerName || "未选择供应商"} · {Number(item.byteSize) / 1024 >= 1024
                    ? (Number(item.byteSize) / 1024 / 1024).toFixed(1) + " MB"
                    : Math.max(1, Math.round(Number(item.byteSize) / 1024)) + " KB"}
                  {item.durationMs != null ? ` · ${Math.round(Number(item.durationMs) / 1000)} 秒` : ""}
                </p>
                <p>
                  {statusText[item.cloneStatus] ?? item.cloneStatus}
                  {item.voiceId ? ` · 音色 ${item.voiceId}` : ""}
                </p>
                {item.cloneError && <p className="service-test-result" data-tone="error">{item.cloneError}</p>}
                <div className="service-actions">
                  <button
                    aria-label={"编辑 " + item.name}
                    disabled={busy}
                    onClick={() => {
                      clearAudio();
                      setReference({
                        id: item.id,
                        name: item.name,
                        providerId: item.providerId ?? "",
                        targetModel: item.targetModel ?? "",
                      });
                      setMessage(
                        `正在编辑「${item.name}」：可修改名称与供应商，保存后原音频与克隆状态保持不变。`,
                      );
                    }}
                  >
                    编辑
                  </button>
                  <button
                    disabled={busy || !item.providerId}
                    title={item.providerId ? undefined : "请先在编辑中选择供应商"}
                    onClick={() => void run(() => api.cloneVoiceReference(item.id), "克隆完成，音色 ID 已保存")}
                  >
                    克隆
                  </button>
                  <button
                    className="button-danger"
                    disabled={busy}
                    onClick={() => void run(() => api.deleteVoiceReference(item.id), "音色已删除")}
                  >
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
