CREATE TABLE practice_plans(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  position TEXT NOT NULL DEFAULT '',
  interviewer_style TEXT NOT NULL DEFAULT '',
  difficulty TEXT NOT NULL DEFAULT 'standard',
  question_count INTEGER NOT NULL DEFAULT 0,
  questions_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (question_count >= 0)
) STRICT;

CREATE TABLE practice_reports(
  id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL DEFAULT '',
  position TEXT NOT NULL DEFAULT '',
  interviewer_style TEXT NOT NULL DEFAULT '',
  total_score REAL NOT NULL DEFAULT 0,
  dimensions_json TEXT NOT NULL DEFAULT '{}',
  objective_json TEXT NOT NULL DEFAULT '{}',
  report_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  FOREIGN KEY (id) REFERENCES sessions(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX idx_practice_reports_created ON practice_reports(created_at);

-- 训练元数据事件：practice_meta 记录题单推进（题号、追问次数、跳过）。
CREATE TABLE session_events_new(
  id INTEGER PRIMARY KEY,
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  CHECK (seq >= 0),
  CHECK (kind IN (
    'status','transcript','reply','takeover',
    'web_sources','turn_meta','scenario','practice_meta'
  )),
  UNIQUE (session_id, seq),
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
) STRICT;

INSERT INTO session_events_new(
  id, session_id, seq, kind, payload, created_at
)
SELECT
  id, session_id, seq, kind, payload, created_at
FROM session_events;

DROP TABLE session_events;
ALTER TABLE session_events_new RENAME TO session_events;
