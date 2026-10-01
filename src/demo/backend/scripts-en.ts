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

/** Debate partner: takes the opposite side (the motion is a fictional example). */
export const DEBATE_SCRIPT_EN: DemoScript = {
  id: "debate",
  turns: [
    {
      userText:
        "Our position: remote work beats office work, because it cuts the commute and lets people put more energy into work and life.",
      replyText:
        "We disagree. Time saved from commuting doesn't automatically become output — focus and collaboration both drop at home, and new hires suffer most. Answer directly: without face-to-face collaboration, how do you fix the slower cross-team decisions?",
    },
    {
      userText:
        "Slower decisions can be solved with async tools — documents first, decisions in writing, which is more transparent than verbal sign-offs.",
      replyText:
        "That transparency is a written illusion: docs-first turns quick alignment into a tug-of-war over wording, and key disagreements explode right before the deadline. Think again — async tools solve \"record-keeping\", not \"consensus\". Don't conflate the two.",
    },
    {
      userText: "Then a compromise: core collaboration days on-site, remote otherwise — balancing focus and collaboration.",
      replyText:
        "The compromise sounds safe, but \"hybrid\" means maintaining two sets of rules: rooms and schedules bend around whoever shows up, and fairness gets harder. Note that your claim has retreated from \"better\" to \"no worse\" — that step alone is our point.",
    },
  ],
};

/** English speaking partner: chat in English plus one-point feedback. */
export const ENGLISH_SCRIPT_EN: DemoScript = {
  id: "english-partner",
  turns: [
    {
      userText: "I have go to a meeting every Monday, and I always feel nervous to speak English in the meeting.",
      replyText:
        "That's completely normal — most people feel that way at first. Quick fix: it's \"I have to go\", and \"nervous to speak\" is more natural as \"nervous about speaking\". Now try another one: what point do you want to make in this week's meeting?",
    },
    {
      userText: "I want to share my opinion about the new design, but I'm afraid my grammar is not well.",
      replyText:
        "Great — that's exactly the kind of sentence you need in meetings. One small thing: \"not well\" should be \"not good\"; grammar is a noun, so it takes an adjective. A confident opener is \"I'd like to share my thoughts on the new design.\" Try saying it in full?",
    },
    {
      userText: "I would like to share my thoughts on the new design. I think the navigation bar should be more simple.",
      replyText:
        "Perfect — natural and confident. One polish: say \"simpler\" instead of \"more simple\"; it sounds more conversational. You can now express a full opinion at work. Next time we'll practice politely disagreeing — deal?",
    },
  ],
};

/** Customer-service drill: complaint → de-escalation → resolution arc. */
export const CS_DRILL_SCRIPT_EN: DemoScript = {
  id: "cs-drill",
  turns: [
    {
      userText:
        "I'm really sorry for the trouble. Let me check the latest tracking status for you — could you give me the order number?",
      replyText:
        "I know the order number by heart! Every call it's the same: give the number, they check, then \"it's being processed\". I don't want \"being processed\" — I want a firm delivery date. Can you find it or can't you?",
    },
    {
      userText:
        "Found it — the package is stuck at a transit hub. Here's what I'll do: request priority dispatch right away, and since it's ten days late, credit you a coupon today as compensation.",
      replyText:
        "Priority dispatch? That's what they said last time! Keep the coupon. Here's the deal: guarantee delivery within two days and we're square. If you can't, I'm returning it for a full refund and you cover the shipping. Do you dare give me a straight answer?",
    },
    {
      userText:
        "I'm writing \"delivery by end of day in two days\" into the ticket with an SMS confirmation. If we miss it: full refund, shipping on us, plus another coupon. Does that work for you?",
      replyText:
        "...Fine. Put it in the ticket exactly like that. To be fair, you've handled this well today — it was the earlier agents who wore me down. And remember: that SMS has to arrive!",
    },
  ],
};

/** Mock candidate: the user interviews; answers deliberately leave follow-up room. */
export const MOCK_CANDIDATE_SCRIPT_EN: DemoScript = {
  id: "mock-candidate",
  turns: [
    {
      userText: "Hi Lin. Start with the project you're most proud of.",
      replyText:
        "Sure. For the past year I worked on an internal data-sync platform, roughly ten million records a day. I owned the failure-retry and alerting modules. Overall, um... the main thing is we didn't lose data. The architecture was set up by my mentor — I contributed to quite a few modules.",
    },
    {
      userText: "\"Didn't lose data\" — how, exactly? What was the retry policy, and didn't retries cause duplicate writes?",
      replyText:
        "Retries used exponential backoff, three attempts max. Duplicates... um, the downstream handled idempotency, deduplicated by business ID. There may have been a tiny chance of losing data, but our scenario allowed occasional backfill, so we didn't adopt a stronger consistency model at the time.",
    },
    {
      userText: "If you could redesign it, what would you change?",
      replyText:
        "I'd pull retry and idempotency into one dedicated transport layer with a reconciliation job as the safety net, instead of scattering them across business modules. That's also why I'm job hunting — I want to see how a mature team handles this at larger scale.",
    },
  ],
};
