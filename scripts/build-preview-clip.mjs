// 一次性生成在线演示的数字人说话片段（提交进仓库，运行时不再联网）。
// 口型帧来自与 digital-human-preview.jpg 同一人物的静帧，台词与界面字幕一致，
// 语音用项目里已有的 msedge-tts（微软 Edge 神经音色）。编码用本机 ffmpeg，不写进依赖。
// 用法：node scripts/build-preview-clip.mjs
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { MsEdgeTTS, OUTPUT_FORMAT } from "msedge-tts";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ffmpeg = process.env.FFMPEG ?? "ffmpeg";
const work = path.join(repoRoot, ".codex-tmp", "preview-clip");
const outFile = path.join(repoRoot, "src", "features", "tavus", "digital-human-preview.mp4");

const LINE1 = "你好，我在。你直接说就行，我看着你听。";
const LINE2 = "需要的话，我可以接着问，也可以先把刚才的话总结给你。";
const GAP_SECONDS = 0.28;
const LISTEN_TAIL_SECONDS = 1.05;
// 张嘴帧落在每个 0.48 秒循环的 0.14–0.34 秒，截图停在 0.32 秒。
const SPEAKING_FRAME_AT = 0.32;

const framesDir = process.env.PREVIEW_FRAMES ?? path.join(repoRoot, "src", "features", "tavus");
const closed = path.join(framesDir, "digital-human-preview.jpg");
const mid = path.join(framesDir, "dh-mouth-mid.jpg");
const open = path.join(framesDir, "dh-mouth-open.jpg");

function run(args) {
  return new Promise((resolve, reject) => {
    const child = spawn(ffmpeg, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve(stderr);
      else reject(new Error(stderr.split("\n").slice(-20).join("\n") || `ffmpeg exited ${code}`));
    });
  });
}

function durationSeconds(ffmpegLog) {
  const match = ffmpegLog.match(/Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)/);
  if (!match) throw new Error("无法从 ffmpeg 输出读取时长");
  return Number(match[1]) * 3600 + Number(match[2]) * 60 + Number(match[3]);
}

await mkdir(work, { recursive: true });

const { rename } = await import("node:fs/promises");
const tts = new MsEdgeTTS();
await tts.setMetadata("zh-CN-XiaoxiaoNeural", OUTPUT_FORMAT.AUDIO_24KHZ_48KBITRATE_MONO_MP3);
const line1 = path.join(work, "line1.mp3");
const line2 = path.join(work, "line2.mp3");
const first = await tts.toFile(work, LINE1);
await rename(first.audioFilePath, line1);
const second = await tts.toFile(work, LINE2);
await rename(second.audioFilePath, line2);

const line1Duration = durationSeconds(await run(["-hide_banner", "-i", line1, "-f", "null", "-"]));
const line2Duration = durationSeconds(await run(["-hide_banner", "-i", line2, "-f", "null", "-"]));
const speechSeconds = line1Duration + GAP_SECONDS + line2Duration;
const totalSeconds = speechSeconds + LISTEN_TAIL_SECONDS;

for (const [name, source] of [["closed.jpg", closed], ["mid.jpg", mid], ["open.jpg", open]]) {
  await run([
    "-y", "-i", source,
    "-vf", "scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720",
    path.join(work, name),
  ]);
}

const cycle = [
  ["mid.jpg", 0.14],
  ["open.jpg", 0.20],
  ["mid.jpg", 0.14],
];
const lines = ["ffconcat version 1.0"];
let cursor = 0;
while (cursor < speechSeconds - 0.02) {
  for (const [file, length] of cycle) {
    const slice = Math.min(length, speechSeconds - cursor);
    if (slice <= 0.02) break;
    lines.push(`file '${file.replaceAll("'", "'\\''")}'`);
    lines.push(`duration ${slice.toFixed(3)}`);
    cursor += slice;
  }
}
lines.push("file 'closed.jpg'");
lines.push(`duration ${LISTEN_TAIL_SECONDS.toFixed(3)}`);
lines.push("file 'closed.jpg'");
await writeFile(path.join(work, "frames.txt"), `${lines.join("\n")}\n`, "utf8");

const audio = path.join(work, "speech.mp3");
await run([
  "-y",
  "-i", line1,
  "-f", "lavfi", "-t", String(GAP_SECONDS), "-i", "anullsrc=r=24000:cl=mono",
  "-i", line2,
  "-f", "lavfi", "-t", String(LISTEN_TAIL_SECONDS), "-i", "anullsrc=r=24000:cl=mono",
  "-filter_complex", "[0:a][1:a][2:a][3:a]concat=n=4:v=0:a=1[a]",
  "-map", "[a]",
  audio,
]);

await run([
  "-y",
  "-f", "concat", "-safe", "0", "-i", path.join(work, "frames.txt"),
  "-i", audio,
  "-vf", "fps=25,format=yuv420p",
  "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", "-crf", "23",
  "-c:a", "aac", "-b:a", "96k",
  "-movflags", "+faststart",
  "-shortest",
  outFile,
]);

const caption2At = line1Duration + GAP_SECONDS;
const listenAt = speechSeconds;
tts.close();

console.log(JSON.stringify({
  line1Duration: Number(line1Duration.toFixed(3)),
  line2Duration: Number(line2Duration.toFixed(3)),
  caption2At: Number(caption2At.toFixed(3)),
  listenAt: Number(listenAt.toFixed(3)),
  totalSeconds: Number(totalSeconds.toFixed(3)),
  speakingFrameAt: SPEAKING_FRAME_AT,
  outFile,
}, null, 2));
