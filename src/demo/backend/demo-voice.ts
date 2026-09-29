// 演示语音（可选）：浏览器标准 speechSynthesis 朗读 AI 回答，零依赖，默认关闭。
// 开关由演示横幅（B06）通过 setDemoVoiceEnabled 控制。
const VOICE_KEY = "roleai.demo.voice";

export function isDemoVoiceEnabled(): boolean {
  try {
    return window.localStorage.getItem(VOICE_KEY) === "1";
  } catch {
    return false;
  }
}

export function setDemoVoiceEnabled(enabled: boolean): void {
  try {
    if (enabled) window.localStorage.setItem(VOICE_KEY, "1");
    else window.localStorage.removeItem(VOICE_KEY);
  } catch {
    // 存储不可用时仅影响本次会话记忆。
  }
  if (!enabled) cancelDemoVoice();
}

export function speakDemoText(text: string): void {
  if (!isDemoVoiceEnabled()) return;
  const synthesis = typeof window !== "undefined" ? window.speechSynthesis : undefined;
  if (!synthesis) return;
  try {
    synthesis.cancel();
    const utterance = new SpeechSynthesisUtterance(text);
    utterance.lang = "zh-CN";
    utterance.rate = 1.05;
    synthesis.speak(utterance);
  } catch {
    // 朗读失败不影响文字时间线。
  }
}

export function cancelDemoVoice(): void {
  try {
    window.speechSynthesis?.cancel();
  } catch {
    // 忽略。
  }
}
