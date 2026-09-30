import { FormEvent, useCallback, useEffect, useRef, useState } from "react";

import * as api from "../../api/commands";
import { EmptyState } from "../../components/empty-state";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { CommandResult, PublicConfig, VoiceReferenceSummary } from "../../generated/bindings";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import { RECORD_MAX_MS, RECORD_MIN_MS, VoiceRecorder, bytesToBase64, type RecordingResult } from "./wav-recorder";

const emptyReference = { id: "", name: "", providerId: "", targetModel: "" };

const optional = (value: string) => value.trim() || null;

const statusKeys: Record<string, DictionaryStringKey> = {
  pending: "services.voices.status.pending",
  uploaded: "services.voices.status.uploaded",
  cloned: "services.voices.status.cloned",
  failed: "services.voices.status.failed",
};

export function VoiceReferenceEditor({ visible }: { visible: boolean }) {
  useT();
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [references, setReferences] = useState<VoiceReferenceSummary[]>([]);
  const [message, setMessage] = useState(() => t("services.voices.reading"));
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
      setMessage(t("services.voices.ipc.readFailed", { error: String(error) }));
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
      setMessage(t("services.voices.ipc.operateFailed", { error: error instanceof Error ? error.message : String(error) }));
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
        reference.id ? t("services.voices.savedWithAudio") : t("services.voices.savedNew"),
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
        t("services.voices.infoUpdated"),
      );
      return;
    }
    setMessage(t("services.voices.needAudioFirst"));
  }

  const startRecording = useCallback(async () => {
    const recorder = new VoiceRecorder();
    try {
      await recorder.start();
    } catch (error) {
      setMessage(t("services.voices.recordStartFailed", { error: error instanceof DOMException ? error.name : String(error) }));
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
          ? t("services.voices.tooShort", { seconds: Math.round(result.durationMs / 1000) })
          : t("services.voices.noAudioCaptured"),
      );
      return;
    }
    setAudio({ base64: result.base64, label: t("services.voices.recordedLabel", { seconds: Math.round(result.durationMs / 1000) }) });
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
      setAudio({ base64: bytesToBase64(bytes), label: t("services.voices.pickedLabel", { name: file.name }) });
      setMessage("");
    } catch {
      setMessage(t("services.voices.fileReadFailed"));
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
      <h2 className="section-heading" id="voice-reference-editor-heading">{t("services.categories.voices")}</h2>
      <p className="configuration-description">
        {t("services.voices.description")}
      </p>
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      <div className="configuration-columns">
        <form className="service-form configuration-editor" onSubmit={submit}>
          <h3>{reference.id && references.some((item) => item.id === reference.id) ? t("services.voices.editTitle") : t("services.voices.addTitle")}</h3>
          <label>
            {t("services.voices.nameLabel")}
            <input
              required
              value={reference.name}
              onChange={(event) => setReference({ ...reference, name: event.target.value })}
            />
          </label>
          <label>
            {t("services.voices.providerLabel")}
            <select
              value={reference.providerId}
              onChange={(event) => setReference({ ...reference, providerId: event.target.value })}
            >
              <option value="">{t("services.voices.providerLaterOption")}</option>
              {providers.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name || t("services.providers.unnamed")}
                </option>
              ))}
            </select>
          </label>
          <label>
            {t("services.voices.targetModelLabel")}
            <input
              value={reference.targetModel}
              onChange={(event) => setReference({ ...reference, targetModel: event.target.value })}
            />
            <small>
              {t("services.voices.targetModelHint")}
            </small>
          </label>
          <div className="voice-recorder-field">
            <span>{t("services.voices.referenceAudioLabel")}</span>
            <span className="voice-recorder">
              {recording ? (
                <button type="button" disabled={busy} onClick={() => void finishRecording()}>
                  {t("services.voices.stopRecording", { elapsed: Math.floor(elapsedMs / 1000) })}
                </button>
              ) : (
                <button type="button" disabled={busy} onClick={() => void startRecording()}>
                  {t("services.voices.startRecording")}
                </button>
              )}
              <button type="button" disabled={busy} onClick={() => fileInputRef.current?.click()}>
                {t("services.voices.pickAudioFile")}
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
                  {t("services.voices.clear")}
                </button>
              </span>
            )}
            {!audio && reference.id && (
              <span className="voice-recorder-status">{t("services.voices.keepExistingNote")}</span>
            )}
            <small>{t("services.voices.fileHint")}</small>
          </div>
          <button
            className="button-primary"
            disabled={busy}
            title={!audio && !reference.id ? t("services.voices.needAudioTitle") : undefined}
            type="submit"
          >
            {audio || !reference.id ? t("services.voices.saveNewTitle") : t("services.voices.saveEditTitle")}
          </button>
        </form>
        <div className="service-list configuration-list">
          <h3>{t("services.voices.listHeading")} <span className="configuration-count">{references.length}</span></h3>
          {references.length === 0 && <EmptyState title={t("services.voices.empty")} />}
          {references.map((item) => {
            const providerName = providers.find((provider) => provider.id === item.providerId)?.name;
            return (
              <article className="service-card" key={item.id}>
                <h3>{item.name}</h3>
                <p>
                  {providerName || t("services.voices.noProvider")} · {Number(item.byteSize) / 1024 >= 1024
                    ? (Number(item.byteSize) / 1024 / 1024).toFixed(1) + " MB"
                    : Math.max(1, Math.round(Number(item.byteSize) / 1024)) + " KB"}
                  {item.durationMs != null ? ` · ${t("services.voices.secondsSuffix", { n: Math.round(Number(item.durationMs) / 1000) })}` : ""}
                </p>
                <p>
                  {statusKeys[item.cloneStatus] ? t(statusKeys[item.cloneStatus]) : item.cloneStatus}
                  {item.voiceId ? ` · ${t("services.voices.voiceIdPrefix", { id: item.voiceId })}` : ""}
                </p>
                {item.cloneError && <p className="service-test-result" data-tone="error">{item.cloneError}</p>}
                <div className="service-actions">
                  <button
                    aria-label={t("services.voices.editAria", { name: item.name })}
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
                        t("services.voices.editingNotice", { name: item.name }),
                      );
                    }}
                  >
                    {t("services.voices.edit")}
                  </button>
                  <button
                    disabled={busy || !item.providerId}
                    title={item.providerId ? undefined : t("services.voices.cloneNeedsProvider")}
                    onClick={() => void run(() => api.cloneVoiceReference(item.id), t("services.voices.cloneDone"))}
                  >
                    {t("services.voices.clone")}
                  </button>
                  <button
                    className="button-danger"
                    disabled={busy}
                    onClick={() => void run(() => api.deleteVoiceReference(item.id), t("services.voices.deleted"))}
                  >
                    {t("services.voices.delete")}
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
