import { Mic, MicOff, PhoneOff } from "lucide-react";

import { t, useT } from "../../i18n";
import { useTavusCall } from "./call-lock";

function statusLabel(status: "listening" | "replica-speaking" | "user-speaking"): string {
  if (status === "replica-speaking") return t("tavus.call.statusReplicaSpeaking");
  if (status === "user-speaking") return t("tavus.call.statusUserSpeaking");
  return t("tavus.call.statusListening");
}

// 通话舞台：媒体元素始终挂载（join 需要它们），可见控件只在通话中出现；
// 空闲时整体 display: contents，不占工作台布局。
export function TavusCall() {
  useT();
  const {
    videoActive,
    callBusy,
    callMessage,
    captions,
    speakingStatus,
    micEnabled,
    setMicEnabled,
    hangup,
    videoRef,
    audioRef,
  } = useTavusCall();
  return (
    <section
      className={videoActive ? "tavus-call" : "tavus-call tavus-call-idle"}
      aria-label={t("tavus.call.stageAria")}
      aria-hidden={!videoActive}
    >
      <video ref={videoRef} autoPlay playsInline muted hidden={!videoActive} />
      <audio ref={audioRef} autoPlay />
      {videoActive && (
        <div className="tavus-call-overlay">
          <div className="tavus-call-captions" aria-live="polite">
            {captions.map((caption, index) => (
              <p key={`${index}-${caption.text}`} data-role={caption.role}>
                {caption.text}
              </p>
            ))}
          </div>
          <p className="tavus-call-status">{statusLabel(speakingStatus)}</p>
          <div className="tavus-call-controls">
            <button
              type="button"
              className="tavus-call-mic"
              data-on={micEnabled}
              aria-pressed={micEnabled}
              aria-label={t("tavus.call.mic")}
              title={t("tavus.call.mic")}
              onClick={() => setMicEnabled(!micEnabled)}
            >
              {micEnabled ? <Mic size={18} aria-hidden="true" /> : <MicOff size={18} aria-hidden="true" />}
            </button>
            <button
              type="button"
              className="tavus-call-hangup"
              disabled={callBusy}
              onClick={() => void hangup(true)}
            >
              <PhoneOff size={16} aria-hidden="true" />
              {t("tavus.call.hangup")}
            </button>
          </div>
          {callMessage && <p className="tavus-call-message" role="status">{callMessage}</p>}
        </div>
      )}
    </section>
  );
}
