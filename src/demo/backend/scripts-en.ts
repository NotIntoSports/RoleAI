// 演示脚本（英文）：与 scripts.ts 的中文版同 id，内容为对应译文。
import type { DemoScript } from "./scripts";

const HANDBOOK = "mat-demo-handbook";

/** Strict interviewer follow-ups (turn 3 demos an interruption). */
export const INTERVIEW_SCRIPT_EN: DemoScript = {
  id: "interview-strict",
  turns: [
    {
      userText:
        "Hello. My most representative project is the messaging service of a collaboration suite — about 400k daily active users, Java plus Redis.",
      replyText:
        "Noted. Let me drill into details first: how do you store read receipts? Why that approach instead of a read table in the database — and how does the memory math work out?",
    },
    {
      userText:
        "We use Redis Bitmaps, bucketed by conversation ID with one bit per member — a few bytes per person per day; buckets idle for thirty days get deleted whole, archived to MySQL first.",
      replyText:
        "Bucketing plus idle eviction — granularity checks out. Now caching: suppose a hot material entry expires and a thousand requests hit the database at once. That's a cache breakdown — how would you handle it?",
    },
    {
      userText:
        "We use logical expiry plus a mutex rebuild: keys never physically expire; on logical expiry only one request rebuilds under a lock while others read the old value. The lock is Redisson with automatic watchdog renewal.",
      replyText:
        "Logical expiry plus singleflight rebuild is the standard play. Let me squeeze the details: if the thread holding the rebuild lock is very slow and requests pile up waiting for the old value, how do you design the timeout layer and the experience layer? Let me split it three ways: first the lock granularity choice, then the waiter's timeout policy, and finally...",
      interruptAfterChars: 52,
    },
    {
      userText:
        "Sorry for interrupting — we actually did a round of optimization on lock granularity, and I can back it with data. May I walk through that case?",
      replyText:
        "Go ahead. A side note: interrupting proactively and bringing your own evidence is a plus in interviews — it shows you have data behind your design. Walk me through it and I'll keep pressing on your case.",
    },
  ],
};

/** Expression coach feedback. */
export const COACH_SCRIPT_EN: DemoScript = {
  id: "coach",
  turns: [
    {
      userText:
        "Um... our project was mainly a messaging service, and, um, DAU was about four hundred thousand, the stack was mostly Java, and, uh, Redis too.",
      replyText:
        "The three issues hurting you most. First, fillers: \"um\" and \"uh\" appeared four times, which undercuts your authority. Second, scattered information: project, scale, and stack are tangled together. Third, no landing point — listeners can't find the center. Rewrite: \"I own the messaging service of our collaboration suite; it carries 400k daily active users on Java and Redis.\" Why: conclusion first, the number gets its own sentence, every filler removed.",
    },
    {
      userText: "Let me try again: I own the messaging service of our collaboration suite; it carries 400k daily active users on Java and Redis.",
      replyText:
        "Much better. One small refinement: pause for half a beat before \"four hundred thousand\" so the number lands: \"I own the messaging service of our collaboration suite — DAU: four hundred thousand.\" This round we focused on fillers; next round, logical connectors.",
    },
    {
      userText: "Got it. How should a three-minute self-introduction be structured?",
      replyText:
        "For three minutes, use four parts: one line of positioning (who you are, years of experience), one representative result (with a number), one proof of ability (a concrete case), one line of role fit (why this job). About forty seconds each. Then record yourself three times, cut at least one \"and then\" each pass, listen back, revise.",
    },
  ],
};

/** Meeting assistant minutes (named-question scenario). */
export const MEETING_SCRIPT_EN: DemoScript = {
  id: "meeting",
  turns: [
    {
      userText: "@meeting assistant what conclusions did we reach today? Give me the minutes.",
      replyText:
        "Key points of this meeting: the discussion covered the v2.3 launch scope and risks. Confirmed facts (per the uploaded product handbook): v2.3 adds cross-team project spaces and offline sync improvements. To confirm: rollout cadence and the rollback window — not mentioned in this discussion yet.",
      materialsUsed: true,
      citations: [
        { materialId: HANDBOOK, snippet: "v2.3 strengthens cross-team project spaces and offline message sync..." },
      ],
    },
    {
      userText: "@meeting assistant what's the current crash rate, and how does it compare with the previous version?",
      replyText:
        "The handbook's \"Security and compliance\" section records: the v2.3 client crash rate (a fictional metric) is 0.12%, down about 40% from v2.2. Defer to the monitoring platform's data for the day; handbook figures may lag.",
      materialsUsed: true,
      citations: [
        { materialId: HANDBOOK, snippet: "The v2.3 client crash rate (a fictional metric) is 0.12%, down about 40% from v2.2." },
      ],
    },
    {
      userText: "@meeting assistant boil today's content down to a three-sentence conclusion.",
      replyText:
        "Today in three sentences: 1. v2.3 ships on time, covering project spaces and offline sync; 2. crash rate 0.12%, down about 40% from the previous version (fictional demo data); 3. rollout and rollback details to be confirmed at the next meeting. Facts follow the uploaded material.",
      materialsUsed: true,
      citations: [
        { materialId: HANDBOOK, snippet: "v2.3 (2026-09) adds project spaces and offline sync improvements..." },
      ],
    },
  ],
};
