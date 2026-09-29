// 可复现的音频评测集生成器（lane-C C41）。
//
// 用 Windows 自带 SAPI（scripts/sapi-tts.ps1）合成面试回答风格的句子，
// 拼装成五类"场景"，并输出逐场景标注 JSON。WAV 与标注都生成到
// target/eval-fixtures/（不入库）；本脚本与句子清单入库。
//
// 运行：node scripts/eval-audio/build-fixtures.mjs
// 确定性：所有时间间隔、噪声增益、混音参数均来自固定种子的 LCG，
// 同一机器上重复运行字节级一致（SAPI 合成波形本身可能逐机略有差异）。

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'target', 'eval-fixtures');
const CACHE_DIR = join(OUT_DIR, 'cache');
const TARGET_RATE = 48_000;

// ---------------------------------------------------------------- LCG ----

function lcg(seed) {
  // Schrage 技巧规避 Math.imul 有符号溢出：保证 state ∈ [1, 0x7ffffffe]，输出 ∈ [0,1)。
  let state = (seed >>> 0) % 0x7fff_ffff || 1;
  return () => {
    const product = Math.imul(state, 48_271);
    state = ((product % 0x7fff_ffff) + 0x7fff_ffff) % 0x7fff_ffff;
    return state / 0x7fff_ffff;
  };
}

// ------------------------------------------------------------ 句子清单 ----

const ANSWER_SENTENCES = [
  '连接池的配置要结合峰值并发和数据库侧的最大连接数来定。',
  '我们先把慢查询日志打开，再决定要不要上调缓冲区。',
  'The retry policy uses exponential backoff capped at thirty seconds.',
  '面试系统的语音链路必须在三十二毫秒内完成一帧处理。',
  '索引的缺失会让对账查询退化成全表扫描，这是首先要确认的。',
  '我们会按天汇总告警，再决定是否需要人工介入。',
  '我建议先压测一轮，拿到真实的分位数据再做容量规划。',
  'Docker 镜像的分层缓存能显著缩短构建时间。',
  '配置项的变更要有审计记录，方便回溯问题。',
  '这个方案的代价是需要额外的一台只读副本。',
  '断路器打开之后，请求会快速失败并进入降级逻辑。',
  '我们把灰度发布分成三批，每批观察半小时。',
  '日志聚合平台能帮我们快速定位跨服务的延迟毛刺。',
  'Under heavy load the queue depth grows and latency follows.',
  '幂等键的设计避免了重试造成的重复下单。',
  '密码学随机数不能用来做业务主键，这一点要区分清楚。',
  '竞态条件的根因是两个线程同时更新同一行状态。',
  '单元测试覆盖不了时序问题，所以还要并发测试。',
  '我们把核心链路的监控做到了秒级采样。',
  '版本回滚的前提是数据库迁移可以双向执行。',
];

const HESITATION_FILLERS = ['嗯，那个……', '这个……怎么说呢。', '呃……让我想一下。'];

// 每个犹豫场景用"前半 + 迟疑 + 后半"拼回一句完整话。
const HESITATION_SENTENCES = [
  ['我们把慢查询的阈值从两百毫秒', '下调到一百毫秒，这样能更早发现问题。'],
  ['The cache layer sits between the service', 'and the database to absorb read spikes.'],
  ['团队先做了两周的埋点', '然后才决定重构哪一段最划算。'],
  ['所以我的结论是先把核心链路的容量', '提升到当前峰值的五倍再观察。'],
];

// AI 播报（用于回声与打断场景）。
const PLAYBACK_SENTENCES = [
  '请介绍一下你在高并发场景下的实践经验。',
  '你如何评估一次重构的风险和收益？',
  'Describe how you would debug a p ninety nine latency regression.',
  '说说你在团队里推动过的最有价值的技术改进。',
];

// ------------------------------------------------------------ WAV 读写 ----

function parseWav(buffer) {
  if (buffer.toString('ascii', 0, 4) !== 'RIFF' || buffer.toString('ascii', 8, 12) !== 'WAVE') {
    throw new Error('not a RIFF/WAVE file');
  }
  let offset = 12;
  let format = null;
  while (offset + 8 <= buffer.length) {
    const id = buffer.toString('ascii', offset, offset + 4);
    const size = buffer.readUInt32LE(offset + 4);
    if (id === 'fmt ') {
      format = {
        channels: buffer.readUInt16LE(offset + 10),
        sampleRate: buffer.readUInt32LE(offset + 12),
        bits: buffer.readUInt16LE(offset + 22),
      };
    } else if (id === 'data') {
      if (!format) throw new Error('fmt chunk must precede data');
      if (format.channels !== 1 || format.bits !== 16) {
        throw new Error(`unsupported wav layout: ${JSON.stringify(format)}`);
      }
      return { sampleRate: format.sampleRate, samples: toSamples(buffer, offset + 8, size) };
    }
    offset += 8 + size + (size % 2);
  }
  throw new Error('data chunk not found');
}

function toSamples(buffer, start, size) {
  const count = Math.min(size, buffer.length - start) >> 1;
  const out = new Float64Array(count);
  for (let i = 0; i < count; i += 1) out[i] = buffer.readInt16LE(start + i * 2) / 32768;
  return out;
}

function linearResample(samples, fromRate, toRate) {
  if (fromRate === toRate) return samples;
  const ratio = fromRate / toRate;
  const outLength = Math.floor(samples.length / ratio);
  const out = new Float64Array(outLength);
  for (let i = 0; i < outLength; i += 1) {
    const src = i * ratio;
    const left = Math.floor(src);
    const right = Math.min(left + 1, samples.length - 1);
    const frac = src - left;
    out[i] = samples[left] * (1 - frac) + samples[right] * frac;
  }
  return out;
}

function writeWav(path, samples, rate) {
  const dataSize = samples.length * 2;
  const buffer = Buffer.alloc(44 + dataSize);
  buffer.write('RIFF', 0, 'ascii');
  buffer.writeUInt32LE(36 + dataSize, 4);
  buffer.write('WAVE', 8, 'ascii');
  buffer.write('fmt ', 12, 'ascii');
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20); // PCM
  buffer.writeUInt16LE(1, 22); // mono
  buffer.writeUInt32LE(rate, 24);
  buffer.writeUInt32LE(rate * 2, 28);
  buffer.writeUInt16LE(2, 32);
  buffer.writeUInt16LE(16, 34);
  buffer.write('data', 36, 'ascii');
  buffer.writeUInt32LE(dataSize, 40);
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i]));
    buffer.writeInt16LE(Math.round(clamped * 32767), 44 + i * 2);
  }
  writeFileSync(path, buffer);
}

function append(builder, samples) {
  for (let i = 0; i < samples.length; i += 1) builder.push(samples[i]);
}

function rms(samples) {
  let sum = 0;
  for (const value of samples) sum += value * value;
  return Math.sqrt(sum / Math.max(1, samples.length));
}

// ------------------------------------------------------------ SAPI 合成 ----

function synthSentence(text) {
  const hash = createHash('sha256').update(text).digest('hex').slice(0, 16);
  const cachePath = join(CACHE_DIR, `${hash}.wav`);
  if (existsSync(cachePath)) return cachePath;
  execFileSync(
    'powershell',
    ['-NoProfile', '-File', join(ROOT, 'scripts', 'sapi-tts.ps1'), '-OutputPath', cachePath],
    { input: text, stdio: ['pipe', 'ignore', 'inherit'] },
  );
  return cachePath;
}

function synthAt48k(text) {
  const raw = parseWav(readFileSync(synthSentence(text)));
  return linearResample(raw.samples, raw.sampleRate, TARGET_RATE);
}

// ---------------------------------------------------------------- 场景 ----

function placeSequence(builder, clips, rng, minGapS, maxGapS) {
  const segments = [];
  for (const clip of clips) {
    const gapS = minGapS + rng() * (maxGapS - minGapS);
    const gap = Math.floor(gapS * TARGET_RATE);
    const start = builder.length + gap;
    append(builder, clip);
    segments.push({ start, end: builder.length, samples: clip });
  }
  return segments;
}

function buildNormalQa(seed) {
  const rng = lcg(seed);
  const builder = [];
  const clips = [];
  for (let i = 0; i < 8; i += 1) {
    clips.push(synthAt48k(ANSWER_SENTENCES[(i * 2 + Math.floor(rng() * 2)) % ANSWER_SENTENCES.length]));
  }
  const speechSegments = placeSequence(builder, clips, rng, 0.8, 2.0);
  return {
    name: 'normal_qa',
    audio: Float64Array.from(builder),
    annotation: {
      scene: 'normal_qa',
      description: '正常问答：8 句回答，句间 0.8–2s 停顿；不期望误打断',
      sampleRate: TARGET_RATE,
      speechSegments: speechSegments.map(({ start, end }) => ({ start, end })),
      expectBargeIn: null,
    },
  };
}

function buildHesitation(seed) {
  const rng = lcg(seed);
  const builder = [];
  const speechSegments = [];
  const hesitationMarks = [];
  for (const [first, second] of HESITATION_SENTENCES) {
    const filler = HESITATION_FILLERS[Math.floor(rng() * HESITATION_FILLERS.length)];
    const firstClip = synthAt48k(first);
    const fillerClip = synthAt48k(filler);
    const secondClip = synthAt48k(second);
    const leadGap = Math.floor((0.9 + rng() * 0.8) * TARGET_RATE);
    const sentenceStart = builder.length + leadGap;
    append(builder, firstClip);
    // 句中迟疑：填充词 + 300–700ms 停顿，期望分段器不在此断句。
    append(builder, fillerClip);
    hesitationMarks.push({ at: builder.length, pauseS: 0.3 + rng() * 0.4 });
    const pauseSamples = Math.floor((0.3 + rng() * 0.4) * TARGET_RATE);
    builder.push(new Float64Array(pauseSamples));
    append(builder, secondClip);
    speechSegments.push({ start: sentenceStart, end: builder.length });
  }
  return {
    name: 'hesitation',
    audio: Float64Array.from(builder),
    annotation: {
      scene: 'hesitation',
      description: '句中犹豫：迟疑填充词 + 300–700ms 句内停顿；期望不产生句中截断',
      sampleRate: TARGET_RATE,
      speechSegments,
      hesitationMarks,
      expectBargeIn: null,
    },
  };
}

function rmsScaleToSnr(speech, snrDb, rng) {
  // 生成与语音等长的白噪声（一阶低通得到轻度粉色成分），按目标 SNR 缩放。
  const noise = new Float64Array(speech.length);
  let previous = 0;
  for (let i = 0; i < noise.length; i += 1) {
    const white = rng() * 2 - 1;
    previous = 0.5 * previous + 0.5 * white; // 轻度粉色
    noise[i] = 0.6 * white + 0.4 * previous;
  }
  const speechRms = rms(speech);
  const noiseRms = rms(noise) || 1e-9;
  const gain = speechRms / 10 ** (snrDb / 20) / noiseRms;
  for (let i = 0; i < noise.length; i += 1) noise[i] *= gain;
  return noise;
}

function buildNoisy(baseSeed, snrDb) {
  const base = buildNormalQa(baseSeed);
  const rng = lcg(baseSeed + snrDb);
  const noise = rmsScaleToSnr(base.audio, snrDb, rng);
  const audio = base.audio.slice();
  for (let i = 0; i < audio.length; i += 1) audio[i] += noise[i];
  return {
    name: `noise_snr${snrDb}`,
    audio,
    annotation: {
      ...base.annotation,
      scene: `noise_snr${snrDb}`,
      description: `正常问答 + 加性噪声 SNR ${snrDb} dB；期望断句行为与 normal_qa 接近`,
    },
  };
}

function buildEcho(seed) {
  const rng = lcg(seed);
  const builder = [];
  const speechSegments = [];
  const echoEvents = [];
  // 结构：AI 播报（经 50–200ms 延迟、-10/-25dB 衰减混入）与用户回答交替。
  for (let round = 0; round < 4; round += 1) {
    const questionClip = synthAt48k(PLAYBACK_SENTENCES[round % PLAYBACK_SENTENCES.length]);
    const answerClip = synthAt48k(ANSWER_SENTENCES[(round * 3) % ANSWER_SENTENCES.length]);
    const playbackStart = builder.length + Math.floor((0.5 + rng()) * TARGET_RATE);
    const delaySamples = Math.floor((0.05 + rng() * 0.15) * TARGET_RATE);
    const gainDb = -(10 + rng() * 15);
    const gain = 10 ** (gainDb / 20);
    // 麦克风路径只包含：用户语音 + 播报的延迟衰减泄漏（回声）。
    // 播报干信号不属于麦克风输入，不进入混音。
    const answerStart = builder.length + Math.floor((0.15 + rng() * 0.2) * TARGET_RATE);
    const answerEnd = answerStart + answerClip.length;
    if (answerEnd > builder.length) append(builder, answerClip);
    speechSegments.push({ start: answerStart, end: Math.min(answerEnd, builder.length) });
    // 回声：播报延迟 + 衰减，混到用户语音期间。
    for (let i = 0; i < questionClip.length; i += 1) {
      const target = playbackStart + delaySamples + i;
      if (target < builder.length) builder[target] += questionClip[i] * gain;
    }
    echoEvents.push({
      playbackStart,
      delayedStart: playbackStart + delaySamples,
      end: playbackStart + delaySamples + questionClip.length,
      gainDb: Number(gainDb.toFixed(1)),
      expectBargeIn: false,
    });
    builder.push(new Float64Array(Math.floor((0.6 + rng() * 0.6) * TARGET_RATE)));
  }
  return {
    name: 'echo',
    audio: Float64Array.from(builder),
    annotation: {
      scene: 'echo',
      description: '回声：AI 播报延迟 50–200ms、衰减 -10~-25dB 混入；期望不触发打断',
      sampleRate: TARGET_RATE,
      speechSegments,
      echoEvents,
      expectBargeIn: false,
    },
  };
}

function buildBargeIn(seed) {
  const rng = lcg(seed);
  const builder = [];
  const events = [];
  for (let round = 0; round < 4; round += 1) {
    const questionClip = synthAt48k(PLAYBACK_SENTENCES[round % PLAYBACK_SENTENCES.length]);
    const answerClip = synthAt48k(ANSWER_SENTENCES[(round * 5 + 1) % ANSWER_SENTENCES.length]);
    const playbackStart = builder.length + Math.floor((0.4 + rng() * 0.4) * TARGET_RATE);
    // 播报经 AEC/房间衰减后仍被麦克风拾到（-12dB）：打断检测的常态输入背景。
    const playbackBleed = questionClip.map((value) => value * 0.25);
    append(builder, playbackBleed);
    // 用户在播报中段真实开口 → 应触发打断。
    const onset = playbackStart + Math.floor(questionClip.length * (0.35 + rng() * 0.25));
    const tail = builder.length - onset;
    if (tail > 0) {
      for (let i = 0; i < answerClip.length; i += 1) {
        const target = onset + i;
        if (target < builder.length) builder[target] = builder[target] * 0.15 + answerClip[i] * 0.9;
        else builder.push(answerClip[i] * 0.9);
      }
    } else {
      append(builder, answerClip);
    }
    events.push({
      playbackStart,
      bargeInOnset: onset,
      expectBargeIn: true,
    });
    builder.push(new Float64Array(Math.floor((0.7 + rng() * 0.5) * TARGET_RATE)));
  }
  return {
    name: 'barge_in',
    audio: Float64Array.from(builder),
    annotation: {
      scene: 'barge_in',
      description: '真打断：AI 播报中段用户开口；期望打断检出且给出检出时刻',
      sampleRate: TARGET_RATE,
      events,
      expectBargeIn: true,
    },
  };
}

// ------------------------------------------------------------------ 主流程 ----

mkdirSync(CACHE_DIR, { recursive: true });
mkdirSync(OUT_DIR, { recursive: true });

const scenes = [
  buildNormalQa(101),
  buildHesitation(202),
  buildNoisy(101, 20),
  buildNoisy(101, 10),
  buildNoisy(101, 5),
  buildEcho(303),
  buildBargeIn(404),
];

const manifest = [];
for (const scene of scenes) {
  const sceneDir = join(OUT_DIR, scene.name);
  mkdirSync(sceneDir, { recursive: true });
  writeWav(join(sceneDir, 'audio.wav'), scene.audio, TARGET_RATE);
  writeFileSync(join(sceneDir, 'annotation.json'), `${JSON.stringify(scene.annotation, null, 2)}\n`);
  manifest.push({
    scene: scene.name,
    audio: join(sceneDir, 'audio.wav'),
    annotation: join(sceneDir, 'annotation.json'),
    durationS: Number((scene.audio.length / TARGET_RATE).toFixed(2)),
  });
  console.log(`built ${scene.name}: ${(scene.audio.length / TARGET_RATE).toFixed(2)}s`);
}
writeFileSync(join(OUT_DIR, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`done: ${manifest.length} scenes in ${OUT_DIR}`);
