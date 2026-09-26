ALTER TABLE sessions ADD COLUMN context_summary TEXT;
ALTER TABLE sessions ADD COLUMN context_summary_turn_index INTEGER NOT NULL DEFAULT -1;
