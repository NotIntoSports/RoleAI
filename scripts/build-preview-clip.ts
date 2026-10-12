// 一次性生成在线演示的会议助手视频（提交进仓库，运行时不再联网）。
// 台词来自 src/features/tavus/preview-script.ts。
// 会议助手的画面走 Tavus 官方视频接口（本机不跑模型）。参会者那一句仍用 msedge-tts。
// 密钥只从环境变量或 Windows 凭据管理器读取，不写进仓库、不打印。
// 编码用本机 ffmpeg。用法：node scripts/build-preview-clip.mjs
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, rename, writeFile } from "node:fs/promises";
import path from "node:path";

import { MsEdgeTTS, OUTPUT_FORMAT } from "msedge-tts";

import {
  PREVIEW_BEATS,
  PREVIEW_USER_VOICE,
  previewBeatSpeech,
  type PreviewBeat,
} from "../src/features/tavus/preview-script";

const repoRoot = process.env.ROLEAI_ROOT ?? process.cwd();
const work = path.join(repoRoot, ".codex-tmp", "preview-clip");
const outFile = path.join(repoRoot, "src", "features", "tavus", "digital-human-preview.mp4");
const timingFile = path.join(repoRoot, "src", "features", "tavus", "preview-timing.json");
const apiOrigin = "https://tavusapi.com";
const MAX_TOTAL_SECONDS = 35;
const VF = "scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,fps=25,format=yuv420p";

const ffmpeg = resolveFfmpeg();

function resolveFfmpeg(): string {
  if (process.env.FFMPEG) return process.env.FFMPEG;
  const bundled = path.join(repoRoot, ".tools", "ffmpeg", "bin", "ffmpeg.exe");
  if (existsSync(bundled)) return bundled;
  return "ffmpeg";
}

function run(bin: string, args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stderr = "";
    child.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
      if (stderr.length > 200_000) stderr = stderr.slice(-100_000);
    });
    child.on("error", (error) => reject(new Error(`无法启动 ${bin}：${error.message}`)));
    child.on("close", (code) => {
      if (code === 0) resolve(stderr);
      else reject(new Error(stderr.split("\n").slice(-30).join("\n") || `${bin} exited ${code}`));
    });
  });
}

function durationSeconds(ffmpegLog: string): number {
  const match = ffmpegLog.match(/Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)/);
  if (!match) throw new Error("无法从 ffmpeg 输出读取时长");
  return Number(match[1]) * 3600 + Number(match[2]) * 60 + Number(match[3]);
}

async function probe(file: string): Promise<number> {
  return durationSeconds(await run(ffmpeg, ["-hide_banner", "-i", file, "-f", "null", "-"]));
}

function encodeArgs(): string[] {
  return [
    "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p", "-r", "25",
    "-c:a", "aac", "-b:a", "128k", "-ar", "48000", "-ac", "2",
  ];
}

async function powershell(script: string): Promise<string> {
  const child = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk: Buffer) => {
    stdout += chunk.toString();
  });
  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  const code = await new Promise<number>((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (exit) => resolve(exit ?? 1));
  });
  if (code !== 0) throw new Error(stderr.trim() || `powershell exited ${code}`);
  return stdout.trim();
}

function alphanumeric(value: string): boolean {
  return value.length >= 8 && value.length <= 128 && /^[A-Za-z0-9]+$/.test(value);
}

async function readTavusKey(): Promise<string> {
  const fromEnv = process.env.TAVUS_API_KEY?.trim() ?? "";
  if (alphanumeric(fromEnv)) return fromEnv;
  const encoded = await powershell(`
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class RoleAiTavusKey {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct CREDENTIAL {
    public uint Flags; public uint Type; public string TargetName; public string Comment;
    public long LastWritten; public uint CredentialBlobSize; public IntPtr CredentialBlob;
    public uint Persist; public uint AttributeCount; public IntPtr Attributes;
    public string TargetAlias; public string UserName;
  }
  [DllImport("advapi32", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern bool CredRead(string target, uint type, uint reserved, out IntPtr credential);
  [DllImport("advapi32")] public static extern void CredFree(IntPtr cred);
  public static string Blob() {
    IntPtr p;
    if (!CredRead("default/tavus/api-key.com.aivirtualassistant.desktop", 1, 0, out p)) return "";
    try {
      var c = Marshal.PtrToStructure<CREDENTIAL>(p);
      if (c.CredentialBlobSize == 0 || c.CredentialBlob == IntPtr.Zero) return "";
      var bytes = new byte[c.CredentialBlobSize];
      Marshal.Copy(c.CredentialBlob, bytes, 0, (int)c.CredentialBlobSize);
      return Convert.ToBase64String(bytes);
    } finally { CredFree(p); }
  }
}
'@
    [RoleAiTavusKey]::Blob()
  `);
  if (!encoded) throw new Error("没有找到已保存的 Tavus 密钥。请先在设置里保存，或设置 TAVUS_API_KEY。");
  const bytes = Buffer.from(encoded, "base64");
  const utf16 = bytes.toString("utf16le").replace(/\0+$/g, "");
  const utf8 = bytes.toString("utf8").replace(/\0+$/g, "");
  const key = alphanumeric(utf16) ? utf16 : utf8;
  if (!alphanumeric(key)) throw new Error("已保存的 Tavus 密钥无法读取。");
  return key;
}

async function tavus(apiKey: string, method: string, pathname: string, body?: unknown): Promise<Record<string, unknown>> {
  let last = "网络中断";
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      const response = await fetch(`${apiOrigin}${pathname}`, {
        method,
        headers: {
          "x-api-key": apiKey,
          ...(body ? { "content-type": "application/json" } : {}),
        },
        body: body ? JSON.stringify(body) : undefined,
      });
      const text = await response.text();
      if (response.status >= 500 || response.status === 429) {
        last = `Tavus ${method} ${pathname} 返回 ${response.status}`;
        await new Promise((resolve) => setTimeout(resolve, 2000 * (attempt + 1)));
        continue;
      }
      if (!response.ok) {
        throw new Error(`Tavus ${method} ${pathname} 返回 ${response.status}：${text.slice(0, 240)}`);
      }
      if (!text) return {};
      return JSON.parse(text) as Record<string, unknown>;
    } catch (error) {
      if (error instanceof Error && error.message.startsWith("Tavus ")) throw error;
      last = error instanceof Error ? error.message : "网络中断";
      await new Promise((resolve) => setTimeout(resolve, 2000 * (attempt + 1)));
    }
  }
  throw new Error(last);
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" ? value as Record<string, unknown> : null;
}

async function replicaId(apiKey: string): Promise<{ id: string; name: string }> {
  const chosen = process.env.TAVUS_REPLICA_ID?.trim();
  const listed = await tavus(apiKey, "GET", "/v2/replicas");
  const data = Array.isArray(listed.data) ? listed.data : [];
  const faces = data.flatMap((item) => {
    const record = asRecord(item);
    const id = typeof record?.replica_id === "string" ? record.replica_id : typeof record?.face_id === "string" ? record.face_id : "";
    const name = typeof record?.replica_name === "string" ? record.replica_name : typeof record?.name === "string" ? record.name : id;
    return id ? [{ id, name }] : [];
  });
  const match = chosen ? faces.find((face) => face.id === chosen) : faces[0];
  if (!match) throw new Error(chosen ? `账号里没有形象 ${chosen}` : "账号里没有可用的 Tavus 形象。");
  return match;
}

async function renderReplica(apiKey: string, replica: string, name: string, script: string): Promise<string> {
  const dest = path.join(work, "tavus", `${name}.mp4`);
  if (process.env.PREVIEW_FORCE !== "1" && existsSync(dest)) {
    console.log(`reuse ${name}`);
    return dest;
  }
  console.log(`tavus ${name}`);
  const created = await tavus(apiKey, "POST", "/v2/videos", {
    replica_id: replica,
    script,
    video_name: `roleai-preview-${name}`,
  });
  const videoId = typeof created.video_id === "string" ? created.video_id : "";
  if (!videoId) throw new Error(`Tavus 没有返回视频编号（${name}）`);
  const deadline = Date.now() + 12 * 60 * 1000;
  let download = "";
  while (Date.now() < deadline) {
    const status = await tavus(apiKey, "GET", `/v2/videos/${videoId}`);
    const state = typeof status.status === "string" ? status.status : "";
    console.log(`tavus ${name} ${state || "waiting"}`);
    if (state === "ready") {
      download = typeof status.download_url === "string" ? status.download_url : "";
      break;
    }
    if (state === "error" || state === "deleted") {
      const detail = typeof status.status_details === "string" ? status.status_details : state;
      throw new Error(`Tavus 生成失败（${name}）：${detail}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 5000));
  }
  if (!download) throw new Error(`Tavus 生成超时（${name}）`);
  let bytes: Buffer | null = null;
  let downloadError = "下载中断";
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      const response = await fetch(download);
      if (!response.ok) throw new Error(`下载 Tavus 视频失败（${name}）：${response.status}`);
      bytes = Buffer.from(await response.arrayBuffer());
      break;
    } catch (error) {
      downloadError = error instanceof Error ? error.message : downloadError;
      await new Promise((resolve) => setTimeout(resolve, 2000 * (attempt + 1)));
    }
  }
  if (!bytes) throw new Error(downloadError);
  await writeFile(dest, bytes);
  return dest;
}

async function synthesizeWithSapi(text: string, destWav: string): Promise<void> {
  const scriptPath = path.join(work, "speak-user.ps1");
  const script = [
    "Add-Type -AssemblyName System.Speech",
    "$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer",
    "$synth.SelectVoice('Microsoft Huihui Desktop')",
    `$synth.SetOutputToWaveFile('${destWav.replaceAll("'", "''")}')`,
    "$synth.Speak(@'",
    text,
    "'@)",
    "$synth.Dispose()",
  ].join("\r\n");
  await writeFile(scriptPath, `\uFEFF${script}`, "utf8");
  await run("powershell.exe", ["-NoProfile", "-NonInteractive", "-File", scriptPath]);
}

async function synthesizeUser(text: string, destWav: string): Promise<void> {
  const mp3 = destWav.replace(/\.wav$/i, ".mp3");
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const tts = new MsEdgeTTS();
    try {
      await tts.setMetadata(PREVIEW_USER_VOICE, OUTPUT_FORMAT.AUDIO_24KHZ_48KBITRATE_MONO_MP3);
      const made = await tts.toFile(path.dirname(destWav), text);
      await rename(made.audioFilePath, mp3);
      tts.close();
      await run(ffmpeg, ["-y", "-i", mp3, "-ac", "1", "-ar", "16000", destWav]);
      return;
    } catch {
      tts.close();
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
  }
  console.log("edge tts unavailable, using installed Chinese voice");
  await synthesizeWithSapi(text, destWav);
}

async function fitSpeech(video: string, out: string): Promise<void> {
  await run(ffmpeg, ["-y", "-i", video, "-vf", VF, ...encodeArgs(), out]);
}

async function listenLoop(source: string, out: string): Promise<void> {
  const duration = await probe(source);
  const tail = Math.min(1.6, Math.max(0.8, duration * 0.18));
  const start = Math.max(0, duration - tail);
  const raw = path.join(work, "listen-raw.mp4");
  await run(ffmpeg, [
    "-y", "-ss", start.toFixed(3), "-i", source, "-t", tail.toFixed(3), "-an",
    "-vf", VF, "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p", "-r", "25",
    raw,
  ]);
  const fade = Math.min(0.25, tail / 3);
  const offset = tail - 2 * fade;
  if (offset <= 0.15) {
    await run(ffmpeg, ["-y", "-i", raw, "-an", "-c", "copy", out]);
    return;
  }
  await run(ffmpeg, [
    "-y", "-i", raw,
    "-filter_complex",
    `[0:v]split[v0][v1];[v0]trim=start=${fade},setpts=PTS-STARTPTS[main];[v1]trim=0:${fade},setpts=PTS-STARTPTS[head];[main][head]xfade=transition=fade:duration=${fade}:offset=${offset.toFixed(3)}[v]`,
    "-map", "[v]", "-an",
    "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p", "-r", "25",
    out,
  ]);
}

async function withSilence(video: string, seconds: number, out: string): Promise<void> {
  await run(ffmpeg, [
    "-y", "-stream_loop", "-1", "-i", video,
    "-f", "lavfi", "-t", seconds.toFixed(3), "-i", "anullsrc=r=48000:cl=stereo",
    "-vf", VF,
    "-map", "0:v:0", "-map", "1:a:0",
    ...encodeArgs(),
    "-t", seconds.toFixed(3),
    out,
  ]);
}

async function loopListen(idle: string, audio: string, out: string): Promise<void> {
  await run(ffmpeg, [
    "-y", "-stream_loop", "-1", "-i", idle, "-i", audio,
    "-vf", VF,
    "-map", "0:v:0", "-map", "1:a:0",
    ...encodeArgs(),
    "-shortest",
    out,
  ]);
}

async function concatSegments(files: string[], out: string): Promise<void> {
  const inputs = files.flatMap((file) => ["-i", file]);
  const pads = files.map((_, index) => `[${index}:v:0][${index}:a:0]`).join("");
  await run(ffmpeg, [
    "-y", ...inputs,
    "-filter_complex", `${pads}concat=n=${files.length}:v=1:a=1[v][a]`,
    "-map", "[v]", "-map", "[a]",
    ...encodeArgs(),
    "-movflags", "+faststart",
    out,
  ]);
}

function replicaBeats(): PreviewBeat[] {
  return PREVIEW_BEATS.filter((beat) => beat.speaker === "replica");
}

function round3(value: number): number {
  return Number(value.toFixed(3));
}

await mkdir(path.join(work, "audio"), { recursive: true });
await mkdir(path.join(work, "tavus"), { recursive: true });
await mkdir(path.join(work, "segments"), { recursive: true });

const apiKey = await readTavusKey();
const replica = await replicaId(apiKey);
console.log(`replica ${replica.name}`);

const rendered = new Map<string, string>();
for (const beat of replicaBeats()) {
  rendered.set(beat.id, await renderReplica(apiKey, replica.id, beat.id, previewBeatSpeech(beat)));
}
const readySource = rendered.get("ready");
const summarySource = rendered.get("summary");
if (!readySource || !summarySource) throw new Error("缺少会议助手的两段视频");

const idle = path.join(work, "idle-loop.mp4");
await listenLoop(readySource, idle);

const askBeat = PREVIEW_BEATS.find((beat) => beat.speaker === "user");
if (!askBeat) throw new Error("剧本里没有参会者");
const askWav = path.join(work, "audio", "ask.wav");
await synthesizeUser(previewBeatSpeech(askBeat), askWav);

const readySeg = path.join(work, "segments", "ready-0-0.mp4");
const summarySeg = path.join(work, "segments", "summary-0-0.mp4");
const askSeg = path.join(work, "segments", "ask-0-0.mp4");
await fitSpeech(readySource, readySeg);
await fitSpeech(summarySource, summarySeg);
await loopListen(idle, askWav, askSeg);

const speech = (await probe(readySeg)) + (await probe(askSeg)) + (await probe(summarySeg));
const room = Math.max(0, MAX_TOTAL_SECONDS - speech - 0.2);
const hold = Math.max(1.2, Math.min(2.5, room / 2));
const lead = path.join(work, "segments", "lead.mp4");
const tail = path.join(work, "segments", "tail.mp4");
await withSilence(idle, hold, lead);
await withSilence(idle, hold, tail);

const segments = [lead, readySeg, askSeg, summarySeg, tail];
const staged = path.join(work, "preview.mp4");
await concatSegments(segments, staged);

const lengths = await Promise.all(segments.map((file) => probe(file)));
let cursor = 0;
const starts: number[] = [];
for (const length of lengths) {
  starts.push(cursor);
  cursor += length;
}
function startOf(prefix: string): number {
  const index = segments.findIndex((file) => path.basename(file).startsWith(prefix));
  if (index < 0) throw new Error(`成片里没有 ${prefix} 段`);
  return starts[index] ?? 0;
}
const readyAt = startOf("ready-");
const askAt = startOf("ask-");
const summaryAt = startOf("summary-");
const listenAt = startOf("tail");
const duration = await probe(staged);
const speakingFrameAt = readyAt + Math.min(0.45, Math.max(0.2, (askAt - readyAt) * 0.25));
if (!(readyAt < askAt && askAt < summaryAt && summaryAt < listenAt && listenAt < duration)) {
  throw new Error(`时间轴顺序不对：${JSON.stringify({ readyAt, askAt, summaryAt, listenAt, duration })}`);
}
if (duration > MAX_TOTAL_SECONDS + 0.35) {
  throw new Error(`成片 ${duration.toFixed(2)}s，超过 ${MAX_TOTAL_SECONDS}s。没有替换旧视频。`);
}

const timing = {
  readyAt: round3(readyAt),
  askAt: round3(askAt),
  summaryAt: round3(summaryAt),
  listenAt: round3(listenAt),
  speakingFrameAt: round3(speakingFrameAt),
  duration: round3(duration),
};
await writeFile(timingFile, `${JSON.stringify(timing, null, 2)}\n`, "utf8");
await copyFile(staged, outFile);
console.log(JSON.stringify({ ...timing, replica: replica.name, outFile }, null, 2));
