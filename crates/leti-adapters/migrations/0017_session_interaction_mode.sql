ALTER TABLE sessions ADD COLUMN interaction_mode TEXT NOT NULL DEFAULT 'interactive';
ALTER TABLE sessions ADD COLUMN detached_on_ask TEXT;
