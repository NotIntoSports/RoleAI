// 演示语音生成器：把全部演示台词经微软 Edge 神经音色预生成为 mp3。
// 产物提交进仓库，演示运行时只播放本地音频（零网络请求）。
// 台词唯一来源：src/demo/backend/{scripts,scripts-en,demo-text}.ts —— 改台词后重跑 `npm run demo:voice:gen`。
// 文件名约定（demo-voice.ts 运行时按约定查找，无清单）：
//   <scriptId>.<zh|en>.t<轮次>.<user|ai>.mp3   脚本台词
//   tail.<zh|en>.<序号>.mp3                    用户插话的兜底回答
import { mkdir, rename, stat } from "node:fs/promises";
import path from "node:path";

import { MsEdgeTTS, OUTPUT_FORMAT } from "msedge-tts";

import { setLanguagePreference } from "../src/i18n";
import { demoT } from "../src/demo/backend/demo-text";
import {
  COACH_SCRIPT,
  CS_DRILL_SCRIPT,
  DEBATE_SCRIPT,
  ENGLISH_SCRIPT,
  INTERVIEW_SCRIPT,
  MEETING_SCRIPT,
  MOCK_CANDIDATE_SCRIPT,
  type DemoScript,
} from "../src/demo/backend/scripts";
import {
  COACH_SCRIPT_EN,
  CS_DRILL_SCRIPT_EN,
  DEBATE_SCRIPT_EN,
  ENGLISH_SCRIPT_EN,
  INTERVIEW_SCRIPT_EN,
  MEETING_SCRIPT_EN,
  MOCK_CANDIDATE_SCRIPT_EN,
} from "../src/demo/backend/scripts-en";

// 音色：用户行男声、AI 行女声；中英各一套（按行内中文字符占比自动选择）。
const ZH_USER = "zh-CN-YunxiNeural";
const ZH_AI = "zh-CN-XiaoxiaoNeural";
const EN_USER = "en-US-BrianNeural";
const EN_AI = "en-US-AvaNeural";

// 输出目录相对仓库根（npm script 的 cwd），不能用 import.meta.url：
// 本文件经 Vite 打包到 .codex-tmp 后执行，import.meta.url 指向 bundle 位置。
const OUT_DIR = path.resolve(process.cwd(), "src/demo/assets/voice");
const FORCE = process.argv.includes("--force");

interface VoiceJob {
  name: string;
  text: string;
  voice: string;
}

function zhRatio(text: string): number {
  const cjk = (text.match(/[\u4e00-\u9fff]/g) ?? []).length;
  const latin = (text.match(/[A-Za-z]/g) ?? []).length;
  if (cjk + latin === 0) return 1;
  return cjk / (cjk + latin);
}

function voiceFor(text: string, side: "user" | "ai"): string {
  if (zhRatio(text) >= 0.3) return side === "user" ? ZH_USER : ZH_AI;
  return side === "user" ? EN_USER : EN_AI;
}

function scriptJobs(script: DemoScript, lang: "zh" | "en"): VoiceJob[] {
  const jobs: VoiceJob[] = [];
  script.turns.forEach((turn, index) => {
    jobs.push({ name: `${script.id}.${lang}.t${index + 1}.user`, text: turn.userText, voice: voiceFor(turn.userText, "user") });
    jobs.push({ name: `${script.id}.${lang}.t${index + 1}.ai`, text: turn.replyText, voice: voiceFor(turn.replyText, "ai") });
  });
  return jobs;
}

/** 用户插话的兜底回答（demoT 按界面语言取值；这些是 AI 的回答，用 AI 音色）。 */
function tailJobs(): VoiceJob[] {
  const jobs: VoiceJob[] = [];
  for (const [lang, tag] of [["zh-CN", "zh"], ["en", "en"]] as const) {
    setLanguagePreference(lang);
    demoT().live.tailReplies.forEach((text, index) => {
      jobs.push({ name: `tail.${tag}.${index + 1}`, text, voice: voiceFor(text, "ai") });
    });
  }
  setLanguagePreference("zh-CN");
  return jobs;
}

async function main(): Promise<void> {
  const jobs = [
    ...scriptJobs(INTERVIEW_SCRIPT, "zh"),
    ...scriptJobs(COACH_SCRIPT, "zh"),
    ...scriptJobs(MEETING_SCRIPT, "zh"),
    ...scriptJobs(DEBATE_SCRIPT, "zh"),
    ...scriptJobs(ENGLISH_SCRIPT, "zh"),
    ...scriptJobs(CS_DRILL_SCRIPT, "zh"),
    ...scriptJobs(MOCK_CANDIDATE_SCRIPT, "zh"),
    ...scriptJobs(INTERVIEW_SCRIPT_EN, "en"),
    ...scriptJobs(COACH_SCRIPT_EN, "en"),
    ...scriptJobs(MEETING_SCRIPT_EN, "en"),
    ...scriptJobs(DEBATE_SCRIPT_EN, "en"),
    ...scriptJobs(ENGLISH_SCRIPT_EN, "en"),
    ...scriptJobs(CS_DRILL_SCRIPT_EN, "en"),
    ...scriptJobs(MOCK_CANDIDATE_SCRIPT_EN, "en"),
    ...tailJobs(),
  ];

  await mkdir(OUT_DIR, { recursive: true });
  const tmpDir = path.join(OUT_DIR, ".tmp");
  await mkdir(tmpDir, { recursive: true });

  // 每种音色复用一个实例，避免频繁重连；顺序生成，不并发打微软端点。
  const ttsByVoice = new Map<string, MsEdgeTTS>();
  async function ttsFor(voice: string): Promise<MsEdgeTTS> {
    let tts = ttsByVoice.get(voice);
    if (!tts) {
      tts = new MsEdgeTTS();
      await tts.setMetadata(voice, OUTPUT_FORMAT.AUDIO_24KHZ_48KBITRATE_MONO_MP3);
      ttsByVoice.set(voice, tts);
    }
    return tts;
  }

  let generated = 0;
  let skipped = 0;
  let totalBytes = 0;
  for (const job of jobs) {
    const target = path.join(OUT_DIR, `${job.name}.mp3`);
    try {
      if (!FORCE && (await stat(target)).size > 0) {
        skipped += 1;
        totalBytes += (await stat(target)).size;
        continue;
      }
    } catch {
      // 文件不存在，继续生成。
    }
    const tts = await ttsFor(job.voice);
    const { audioFilePath } = await tts.toFile(tmpDir, job.text);
    await rename(audioFilePath, target);
    generated += 1;
    totalBytes += (await stat(target)).size;
    console.log(`✓ ${job.name}.mp3 (${job.voice})`);
  }

  console.log(`完成：生成 ${generated} 条，跳过已有 ${skipped} 条，共 ${(totalBytes / 1024 / 1024).toFixed(2)} MB → src/demo/assets/voice/`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
