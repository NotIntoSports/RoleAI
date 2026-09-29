// 脚本化实时会话时间线：按脚本驱动 transcript/reply/status 事件，
// 事件名与载荷与真实后端一致（见 use-session-events.ts / bindings.ts）：
// - runtime:status:v1 → RuntimeStatus
// - session:transcript:v1 / session:reply:v1 → {seq, text, done}
// - session:playback-control:v1 → {seq, action:"clear"}（打断）
// 可注入 emitEvent，单测用假时钟驱动；脚本轮与用户插话共用一条串行队列。
import type { RuntimeStatus } from "../../generated/bindings";

import type { DemoScript, DemoScriptTurn } from "./scripts";
import { cancelDemoVoice, isDemoVoiceEnabled, speakDemoText } from "./demo-voice";

const OPENING_DELAY_MS = 700;
const THINKING_MS: [number, number] = [450, 900];
const USER_CHUNK_MS: [number, number] = [60, 120];
const REPLY_CHUNK_MS: [number, number] = [30, 70];
/** 打断后到下一条用户发言的停顿。 */
const INTERRUPT_GAP_MS = 900;
const TURN_GAP_MS = 500;

const CJK_CHAR = /[\u4e00-\u9fff\u3001-\u303f\uff01-\uff5e“”‘’…—]/;
const CJK_OR_WORD = new RegExp(
  `${CJK_CHAR.source}|[A-Za-z0-9][A-Za-z0-9'%.\\-]*\\s*|\\s+`,
  "g",
);

/** 把文本切成适合逐字上屏的片段：CJK 按字（2 字一组），拉丁按词。 */
export function chunkText(text: string): string[] {
  if (!text) return [];
  const parts = text.match(CJK_OR_WORD) ?? [text];
  const chunks: string[] = [];
  let cjkBuffer = "";
  const pushChunk = (chunk: string) => {
    if (/^\s+$/.test(chunk)) {
      // 空白跟随前一个片段输出（保持累计文本与原文一致）；行首空白丢弃。
      const previous = chunks[chunks.length - 1];
      if (previous) chunks[chunks.length - 1] = previous + chunk;
      return;
    }
    chunks.push(chunk);
  };
  for (const part of parts) {
    if (part.length === 1 && CJK_CHAR.test(part)) {
      cjkBuffer += part;
      if (cjkBuffer.length >= 2) {
        pushChunk(cjkBuffer);
        cjkBuffer = "";
      }
    } else {
      if (cjkBuffer) {
        pushChunk(cjkBuffer);
        cjkBuffer = "";
      }
      pushChunk(part);
    }
  }
  if (cjkBuffer) pushChunk(cjkBuffer);
  return chunks;
}

export interface ScriptedLiveSessionDeps {
  emitEvent: (event: string, payload: unknown) => void | Promise<void>;
  random?: () => number;
}

/** 持久化钩子由 live-commands 注入（落库到 records 的会话状态）。 */
export interface LivePersistence {
  appendTurn: (sessionId: string, userText: string, assistantText: string) => void;
  updateTurnAssistant: (
    sessionId: string,
    userText: string,
    assistantText: string,
    extras?: { materialsUsed?: boolean; citations?: LiveTurn["citations"] },
  ) => void;
  finish: (sessionId: string) => void;
}

interface LiveTurn {
  userText: string;
  assistantText: string;
  citations?: Array<{ materialId: string; chunkId: string; snippet: string }>;
  materialsUsed?: boolean;
}

export class ScriptedLiveSession {
  readonly sessionId: string;
  private readonly script: DemoScript;
  private readonly emitEvent: ScriptedLiveSessionDeps["emitEvent"];
  private readonly random: () => number;
  private readonly persistence: LivePersistence;

  private disposed = false;
  private paused = false;
  private resumeWaiters: Array<() => void> = [];
  private timerCancels = new Set<() => void>();
  private chain: Promise<void> = Promise.resolve();

  private statusSeq = 0;
  private transcriptSeq = 0;
  private replySeq = 0;
  private playbackSeq = 0;
  private phase: "listening" | "thinking" | "speaking" = "listening";
  private mode = "ai_active";
  private revision = 0;
  private lastTurn: LiveTurn | null = null;
  private tailCursor = 0;

  constructor(
    sessionId: string,
    script: DemoScript,
    deps: ScriptedLiveSessionDeps,
    persistence: LivePersistence,
  ) {
    this.sessionId = sessionId;
    this.script = script;
    this.emitEvent = deps.emitEvent;
    this.random = deps.random ?? Math.random;
    this.persistence = persistence;
  }

  // —— 生命周期 ——

  /** 开始播放脚本（后台运行，串行队列保证事件有序）。 */
  begin(): void {
    void (async () => {
      await this.emitStatus("listening");
      await this.sleep(OPENING_DELAY_MS);
      for (const turn of this.script.turns) {
        if (this.disposed) return;
        await this.enqueue(() => this.runScriptedTurn(turn));
      }
    })().catch(() => {
      /* 时间线绝不抛未捕获异常 */
    });
  }

  /** 串行执行：脚本轮与用户插话共用一条队列，避免事件乱序。 */
  private enqueue(step: () => Promise<void>): Promise<void> {
    const run = this.chain.then(step).catch(() => undefined);
    this.chain = run;
    return run;
  }

  setMode(mode: string): void {
    this.mode = mode;
    if (mode !== "ai_active") {
      cancelDemoVoice();
      this.paused = true;
    } else {
      this.paused = false;
      this.resumeWaiters.splice(0).forEach((wake) => wake());
    }
  }

  stop(): void {
    if (this.disposed) return;
    this.disposed = true;
    cancelDemoVoice();
    for (const cancel of [...this.timerCancels]) cancel();
    this.timerCancels.clear();
    this.paused = false;
    this.resumeWaiters.splice(0).forEach((wake) => wake());
    this.persistence.finish(this.sessionId);
    this.statusSeq += 1;
    void this.emitEvent("runtime:status:v1", this.statusPayload("idle"));
  }

  /** 用户文字插话（聊天输入 / 让助手回答）。 */
  submitUserText(text: string): void {
    const trimmed = text.trim();
    if (!trimmed || this.disposed) return;
    void this.enqueue(async () => {
      if (this.disposed) return;
      await this.runUserTurn(trimmed);
    });
  }

  /** 重答：把上一条用户发言的回答重新流式输出一遍。 */
  retryLast(): void {
    if (!this.lastTurn || this.disposed) return;
    void this.enqueue(async () => {
      if (this.disposed || !this.lastTurn) return;
      const retry: LiveTurn = { ...this.lastTurn };
      await this.emitStatus("thinking");
      await this.pauseBetween(this.randMs(THINKING_MS));
      if (this.disposed) return;
      await this.emitStatus("speaking");
      await this.streamReplyChunks(retry, retry.assistantText || "（重答）好的，我们再讲一遍。", null);
      if (this.disposed) return;
      await this.emitStatus("listening");
    });
  }

  getStatus(): RuntimeStatus {
    return this.statusPayload(this.disposed ? "idle" : this.phase);
  }

  // —— 时间线细节 ——

  private statusPayload(phase: string): RuntimeStatus {
    return {
      phase,
      mode: this.mode,
      seq: this.statusSeq,
      unusedMaterials: false,
      lastErrorCode: null,
      revision: this.revision,
      realtimeStatus: this.disposed ? "idle" : "connected",
    };
  }

  private async emitStatus(phase: "listening" | "thinking" | "speaking" | "idle"): Promise<void> {
    if (phase !== "idle") this.phase = phase;
    this.statusSeq += 1;
    await this.emitEvent("runtime:status:v1", this.statusPayload(phase));
  }

  private randMs([min, max]: [number, number]): number {
    return min + Math.floor(this.random() * (max - min + 1));
  }

  private async gate(): Promise<void> {
    while (this.paused && !this.disposed) {
      await new Promise<void>((resolve) => this.resumeWaiters.push(resolve));
    }
  }

  private sleep(ms: number): Promise<void> {
    return new Promise<void>((resolve) => {
      if (this.disposed) {
        resolve();
        return;
      }
      const timer = setTimeout(() => {
        this.timerCancels.delete(cancel);
        resolve();
      }, ms);
      const cancel = () => {
        clearTimeout(timer);
        this.timerCancels.delete(cancel);
        resolve();
      };
      this.timerCancels.add(cancel);
    });
  }

  private async pauseBetween(ms: number): Promise<void> {
    await this.gate();
    await this.sleep(ms);
    await this.gate();
  }

  /** 用户发言：转写逐字上屏 → 落库 → done。 */
  private async runUserSpeech(userText: string): Promise<void> {
    await this.emitStatus("listening");
    const chunks = chunkText(userText);
    let emitted = "";
    for (const chunk of chunks) {
      emitted += chunk;
      this.transcriptSeq += 1;
      await this.emitEvent("session:transcript:v1", {
        seq: this.transcriptSeq,
        text: emitted,
        done: false,
      });
      await this.pauseBetween(this.randMs(USER_CHUNK_MS));
      if (this.disposed) return;
    }
    this.persistence.appendTurn(this.sessionId, userText, "");
    this.revision += 1;
    this.transcriptSeq += 1;
    await this.emitEvent("session:transcript:v1", {
      seq: this.transcriptSeq,
      text: emitted,
      done: true,
    });
  }

  /** 一轮完整脚本：用户发言 → 思考 → 回答（第 3 轮可被打断）。 */
  private async runScriptedTurn(turn: DemoScriptTurn): Promise<void> {
    await this.gate();
    if (this.disposed) return;
    await this.runUserSpeech(turn.userText);
    if (this.disposed) return;
    await this.emitStatus("thinking");
    await this.pauseBetween(this.randMs(THINKING_MS));
    if (this.disposed) return;
    await this.emitStatus("speaking");
    const pending: LiveTurn = {
      userText: turn.userText,
      assistantText: "",
      materialsUsed: turn.materialsUsed,
      citations: turn.citations?.map((citation, index) => ({
        ...citation,
        chunkId: `${citation.materialId}-live-${index}`,
      })),
    };
    if (isDemoVoiceEnabled()) speakDemoText(turn.replyText);
    const cut = turn.interruptAfterChars ?? null;
    const streamed = await this.streamReplyChunks(pending, turn.replyText, cut);
    if (cut !== null && streamed.length < turn.replyText.length) {
      // 用户打断：清播放队列、回聆听态；截断的回答已由 streamReplyChunks 落库。
      cancelDemoVoice();
      this.playbackSeq += 1;
      await this.emitEvent("session:playback-control:v1", {
        seq: this.playbackSeq,
        action: "clear",
      });
      await this.emitStatus("listening");
      await this.pauseBetween(INTERRUPT_GAP_MS);
      return;
    }
    await this.emitStatus("listening");
    await this.pauseBetween(TURN_GAP_MS);
  }

  /** 用户文字插话：落库并回答（脚本耗尽后用兜底回复）。 */
  private async runUserTurn(text: string): Promise<void> {
    const reply = SCRIPT_TAIL[this.tailCursor % SCRIPT_TAIL.length];
    this.tailCursor += 1;
    await this.runUserSpeech(text);
    if (this.disposed) return;
    await this.emitStatus("thinking");
    await this.pauseBetween(this.randMs(THINKING_MS));
    if (this.disposed) return;
    await this.emitStatus("speaking");
    if (isDemoVoiceEnabled()) speakDemoText(reply);
    await this.streamReplyChunks({ userText: text, assistantText: "" }, reply, null);
    if (this.disposed) return;
    await this.emitStatus("listening");
  }

  /**
   * 回答流式上屏；完成后把全文写入 pending 并落库（done）。
   * cutAfterChars 非空时在截断点停止（打断演示），落库文本为截断内容。
   */
  private async streamReplyChunks(pending: LiveTurn, replyText: string, cutAfterChars: number | null): Promise<string> {
    const chunks = chunkText(replyText);
    let emitted = "";
    for (const chunk of chunks) {
      const candidate = emitted + chunk;
      if (cutAfterChars !== null && candidate.length > cutAfterChars) break;
      emitted = candidate;
      this.replySeq += 1;
      await this.emitEvent("session:reply:v1", {
        seq: this.replySeq,
        text: emitted,
        done: false,
      });
      await this.pauseBetween(this.randMs(REPLY_CHUNK_MS));
      if (this.disposed) return emitted;
    }
    pending.assistantText = emitted;
    this.persistence.updateTurnAssistant(this.sessionId, pending.userText, emitted, {
      materialsUsed: pending.materialsUsed,
      citations: pending.citations,
    });
    this.revision += 1;
    this.lastTurn = pending;
    this.replySeq += 1;
    await this.emitEvent("session:reply:v1", { seq: this.replySeq, text: emitted, done: true });
    return emitted;
  }
}

const SCRIPT_TAIL = [
  "演示脚本到这里就播完了。桌面的真实版本会持续进行语音对话；在线演示里你可以输入文字继续体验，或结束会话回看记录。",
  "这段是演示的固定结尾：想再看一遍完整脚本，可以结束会话后重新开始；想体验真实语音，请下载桌面版。",
];
