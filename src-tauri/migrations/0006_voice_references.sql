CREATE TABLE voice_references(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  provider_id TEXT,
  mime_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  duration_ms INTEGER,
  transcript TEXT NOT NULL DEFAULT '',
  audio BLOB NOT NULL,
  remote_file_id TEXT,
  voice_id TEXT,
  clone_status TEXT NOT NULL DEFAULT 'pending',
  clone_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (byte_size >= 1 AND byte_size <= 10485760),
  CHECK (clone_status IN ('pending', 'uploaded', 'cloned', 'failed'))
) STRICT;
