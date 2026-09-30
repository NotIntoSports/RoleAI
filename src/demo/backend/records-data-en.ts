// 演示会话记录种子（英文）：与 records-data.ts 的中文版同 id、同角色、同传输模式，内容为对应译文。
import type { DemoSeedSession } from "./records-data";

export const DEMO_SESSIONS_EN: DemoSeedSession[] = [
  {
    id: "session-demo-interview-backend",
    roleProfileId: "preset-strict-interviewer",
    status: "completed",
    transportMode: "realtime-e2e",
    startedAt: "2026-09-27T19:30:00+08:00",
    finishedAt: "2026-09-27T19:58:00+08:00",
    turns: [
      {
        user: "Hello. At Yunfan Tech I own the messaging service of our collaboration suite — about 400k daily active users, mostly Java, Redis, and Kafka.",
        assistant:
          "Got it. First follow-up: how do you store read receipts in your messaging service? Why that design instead of a plain read table in the database?",
      },
      {
        user: "We use Redis Bitmaps for read receipts, bucketed by conversation ID, one bit per member — a few bytes per person per day, far cheaper than row storage.",
        assistant:
          "Bitmaps are indeed memory-efficient. Follow-up: when a member leaves a group, the bits in that bucket never shrink. Doesn't memory waste accumulate over time? How do you handle it?",
      },
      {
        user: "Buckets have an idle-eviction policy: after 30 days without writes the whole bucket is deleted, and read progress is archived to MySQL first, restored if the member returns.",
        assistant:
          "Archive plus whole-bucket deletion — the granularity checks out. Now caching: your material queries have a Redis cache in front. If a hot cache entry expires and a thousand requests hit the database at once, what happens? How do you handle it?",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-article", snippet: "A cache breakdown happens the instant a single hot key expires and many concurrent requests hit the database at once..." },
        ],
      },
      {
        user: "That's cache breakdown. We do two layers: hot keys use logical expiry so they never physically expire; on logical expiry a mutex rebuild lets exactly one request rebuild while others read the old value.",
        assistant:
          "Logical expiry plus singleflight rebuild is the standard approach. Let me push harder: if the thread holding the rebuild lock stalls or the origin is very slow, and the lock has no timeout, what happens? How is your lock set up?",
      },
      {
        user: "We use Redisson's distributed lock — its watchdog renews the lease automatically, so there is no permanent deadlock; rebuild failures have a retry cap, and past the cap we degrade to serving the old value.",
        assistant:
          "The watchdog renewal is accurate. Next: how do you defend against penetration and avalanche? Put them together with breakdown into one layered plan — no scattered bullets, please.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-article", snippet: "Cache penetration is querying data that does not exist, so every request reaches the database. Defenses: a Bloom filter..." },
        ],
      },
      {
        user: "Penetration: a Bloom filter at the entry rejects missing keys outright, and empty results are cached for sixty seconds; avalanche: jittered expiry times plus a local first-level cache, with rate limiting as the backstop on the origin path.",
        assistant:
          "The layering is clear: entry filtering, empty-value caching, TTL jitter, multi-level cache, rate-limited fallback — each layer has a distinct job. One last follow-up: if your DBA tells you replication lag is three seconds, which layer of your cache-consistency plan breaks first, and why?",
      },
      {
        user: "The first thing to break would be the update-DB-then-evict window: the replica hasn't caught up, another request reads stale replica data and writes it back into the cache. We'd move to binlog-subscription eviction, with a short TTL as the backstop during lag.",
        assistant:
          "Right: downgrade strong consistency to eventual consistency with a TTL backstop, and you called out \"stale write-back\", the real trap. That's it for the backend portion. One thought exercise: if DAU grows tenfold, which part of your Bitmap bucketing fails first? We'll pick it up there next time.",
      },
      {
        user: "I'd guess the bitmap granularity: bits can't be evicted per member, only whole buckets, so bigger buckets waste more — we'd need small per-member shards, or HyperLogLog plus exact spot checks.",
        assistant:
          "Correct — the bitmap's granularity is the bit, not the person; that's its natural boundary. This round's assessment: Redis data-structure choices (excellent), breakdown/penetration/avalanche layering (excellent), proactive thinking about consistency boundaries (good). Your solutions land on concrete components and the trade-offs are clear. Expect the next round's schedule; I'd brush up on Kafka ordering.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-jd", snippet: "Solid understanding of Redis, MySQL, and Kafka internals and tuning..." },
        ],
      },
    ],
  },
  {
    id: "session-demo-interview-project",
    roleProfileId: "preset-strict-interviewer",
    status: "completed",
    transportMode: "realtime-e2e",
    startedAt: "2026-09-28T20:00:00+08:00",
    finishedAt: "2026-09-28T20:26:00+08:00",
    turns: [
      {
        user: "Since the last round I studied Kafka. Today I'd like you to drill into the flash-sale project on my resume.",
        assistant:
          "Fine, let's talk flash sales. Scene setting: ten thousand items in stock, two hundred thousand requests at once — how does your entry layer cope? Start with your rate-limiting algorithm choice and why.",
      },
      {
        user: "The entry layer uses token buckets at the gateway, keyed by user ID and IP; below that, per-node sliding-window counters smooth out bursts.",
        assistant:
          "Token buckets for bursts, sliding windows for spikes — that combination holds up. Follow-up: how did you pick the token-bucket rate — gut feeling or evidence? If ops moves the sale ten minutes earlier, can your limits be changed in time?",
      },
      {
        user: "The rate is set at eighty percent of load-test capacity, stored in the config center — changes take effect in seconds; there's also a master switch that diverts the sale to a queue page.",
        assistant:
          "Load-test evidence plus a config center — that answer clears the bar. Next: no overselling allowed. A thousand concurrent requests race for the last item — how do you write the deduction logic?",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-jd", snippet: "Familiar with common distributed-systems problems (consistency, idempotency, rate limiting, graceful degradation)..." },
        ],
      },
      {
        user: "Stock is pre-warmed into Redis; a Lua script makes read-check-deduct-write atomic, and only a successful deduction publishes to the queue for the database write, with an optimistic version lock as the last line of defense.",
        assistant:
          "Atomic Redis deduction, async queue write, optimistic lock backstop — the layers are right. How do you guarantee Redis and the database eventually agree? If a queue consumer fails or the service crashes midway and orders and stock disagree, what then?",
      },
      {
        user: "Consumer failures go to a retry queue, then a dead-letter queue for manual handling; a reconciliation job compares Redis deduction flows against the orders table every five minutes and auto-compensates differences.",
        assistant:
          "Reconciliation and compensation — that tells me you've handled real incidents. Next: the same user double-clicks or the network retries — could one item produce two orders? What's your idempotency scheme?",
      },
      {
        user: "The idempotency key is user ID plus activity ID, with a unique index at write time; requests first check a Redis dedup set and get the previous result on a hit.",
        assistant:
          "The unique index as the last line of defense — at the database, not just Redis dedup — that's the key point, and you got it. Final question: two hundred thousand requests arrive but the database can only absorb five thousand TPS. What do you do with the queue backlog?",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-article", snippet: "Apply rate limiting and fallback plans on the origin path..." },
        ],
      },
      {
        user: "During the sale, consumers only do the lightest persistence; non-critical paths (points, notifications) go async or degrade to off; past the backlog watermark we scale consumer partitions, and everything is drained before the sale ends.",
        assistant:
          "Protect the critical path, yield the non-critical one — the priority is right. This round's assessment: rate-limit choices (excellent), atomic stock deduction (excellent), idempotency and reconciliation (excellent), capacity and fallback planning (good). Across both rounds you show real feel for system boundaries; push your reconciliation automation one level further.",
      },
      {
        user: "Thank you — I'll draft a fuller reconciliation-automation proposal after this.",
        assistant:
          "My pleasure. Turn every follow-up into a checklist item — that's what interview training is for. One note before we finish: your pacing is good, but your speech speeds up noticeably under follow-ups. Pause deliberately and leave room to think. Next time: system design.",
      },
    ],
  },
  {
    id: "session-demo-meeting-notes",
    roleProfileId: "preset-meeting",
    status: "completed",
    transportMode: "meeting-bridge",
    startedAt: "2026-09-29T10:00:00+08:00",
    finishedAt: "2026-09-29T10:47:00+08:00",
    turns: [
      {
        user: "Start recording the product review. Today's topic is the v2.3 launch scope for the Yunfan platform.",
        assistant: "Recording. Topic: v2.3 launch scope. @-mention the meeting assistant to ask questions.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3 (2026-09) adds project spaces and offline sync improvements..." },
        ],
      },
      {
        user: "@meeting assistant what is the v2.3 client crash rate compared with v2.2?",
        assistant:
          "Per the product handbook: the v2.3 client crash rate (a fictional metric) is 0.12%, down about 40% from v2.2. The figure comes from the handbook's \"Security and compliance\" section; defer to the monitoring platform's data for the day.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "The v2.3 client crash rate (a fictional metric) is 0.12%, down about 40% from v2.2." },
        ],
      },
      {
        user: "@meeting assistant what are the limitations when editing documents offline?",
        assistant:
          "The handbook's \"Release notes\" section states: sheet functions do not recalculate automatically when editing offline; other editing features work. Whether that affects this launch scope should be confirmed with the documents team.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "Known limitation: sheet functions do not recalculate automatically when editing documents offline." },
        ],
      },
      {
        user: "Let's go over the risk list: the offline-sync rework touches three platforms; the rollback plan keeps the v2.2 sync channel for two weeks.",
        assistant: "Noted: the offline-sync rework touches three platforms; rollback keeps the v2.2 sync channel for two weeks.",
      },
      {
        user: "@meeting assistant summarize today's conclusions in three sentences.",
        assistant:
          "Today's conclusions: 1. v2.3 ships on time, including project spaces and offline sync improvements; 2. crash rate 0.12% (down about 40% vs v2.2, fictional demo data); 3. rollback keeps the v2.2 sync channel for two weeks. Facts follow the uploaded material; unconfirmed items are flagged.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3 (2026-09) adds project spaces and offline sync improvements; v2.2 (2026-06) added the meeting assistant and audit export..." },
        ],
      },
      {
        user: "Before we close: who owns acceptance of the audit-log export feature?",
        assistant: "This turn did not name an acceptance owner; recorded as-is: \"owner of audit-log export acceptance: to be confirmed\".",
      },
      {
        user: "Adding to the schedule: full gradual rollout next Tuesday, rollout retrospective on Thursday.",
        assistant: "Schedule recorded: full rollout next Tuesday; retrospective on Thursday.",
      },
      {
        user: "@meeting assistant how long are audit logs kept, and can they be exported to external platforms?",
        assistant:
          "Per the product handbook: audit logs are kept for 365 days and can be exported to SIEM platforms. That is from the \"Security and compliance\" section; confirm delivery details with the security team.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "Audit logs are kept for 365 days and can be exported to SIEM platforms." },
        ],
      },
    ],
  },
  {
    id: "session-demo-livestream",
    roleProfileId: "preset-presenter",
    status: "completed",
    transportMode: "realtime-e2e",
    startedAt: "2026-09-26T15:00:00+08:00",
    finishedAt: "2026-09-26T15:22:00+08:00",
    turns: [
      {
        user: "Starting the livestream rehearsal. Today we present the Yunfan collaboration suite, using the uploaded product handbook.",
        assistant:
          "Rehearsal started. The script will follow the \"Yunfan Collaboration Suite Product Handbook v2.3\" strictly — no invented prices, stock, or promotions. Segment one: the opening.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "Yunfan Collaboration Suite (a fictional product) serves companies with 50–5,000 people..." },
        ],
      },
      {
        user: "How should the opening go? Give me a thirty-second script.",
        assistant:
          "\"Hi everyone — today we're looking at the Yunfan collaboration suite: messaging, documents, schedules, and approvals on one platform. Teams from fifty to five thousand people can use it, with clients on Windows, macOS, and the browser. Today we focus on v2.3's new project spaces and offline sync.\" (about thirty seconds)",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "v2.3 strengthens cross-team project spaces and offline message sync..." },
        ],
      },
      {
        user: "Someone in chat asks: how long are offline messages kept?",
        assistant:
          "Answering chat: per the handbook, offline messages stay on the server for ninety days and are pulled incrementally per conversation when you're back online. Only what the handbook states — nothing more.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "Offline messages stay on the server for 90 days and are pulled incrementally per conversation when you come back online." },
        ],
      },
      {
        user: "Now walk through the document collaboration selling points.",
        assistant:
          "\"On documents: real-time co-editing without conflicts, version history kept for one hundred and eighty days so you can always roll back, sheets with common functions and pivot tables, and one-click conversion of a document into a review task with comments rolling up into the project space.\"",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "Online documents support real-time multi-user editing and version history kept for 180 days..." },
        ],
      },
      {
        user: "Someone asks about pricing — what do I say?",
        assistant:
          "Deflect pricing per the compliance line: \"Pricing and promotions follow the official site and our sales team; we don't quote prices in the livestream — contact sales for a quote.\" The handbook contains no pricing, and the presenter invents none.",
      },
      {
        user: "How do I present security and compliance?",
        assistant:
          "\"One more word on security: TLS 1.3 end to end, per-tenant encryption at rest, admin-configurable watermarks, outbound approval, and device allowlists, plus a full year of audit logs that can feed your own audit platform.\"",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "TLS 1.3 covers the whole transport path; storage is encrypted per tenant..." },
        ],
      },
      {
        user: "Chat asks how to integrate the audit logs with their company's platform.",
        assistant:
          "Answering chat: the handbook states audit logs are kept 365 days and support SIEM export; for integration details, leave your contact so the solutions team can share the docs — we won't go into technical detail in the livestream.",
        materialsUsed: true,
        citations: [
          { materialId: "mat-demo-handbook", snippet: "Audit logs are kept for 365 days and can be exported to SIEM platforms." },
        ],
      },
      {
        user: "Let's wrap it up.",
        assistant:
          "\"To sum up: cross-team project spaces, offline sync, and audit compliance — the three keywords of v2.3. Download the client from the official site to try it; next livestream we'll do a hands-on document collaboration walkthrough.\"",
      },
    ],
  },
];
