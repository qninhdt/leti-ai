CREATE TABLE subagent_inbox_messages (
  id              TEXT PRIMARY KEY,
  task_id         TEXT NOT NULL REFERENCES subagent_executions(task_id) ON DELETE CASCADE,
  root_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  sender          TEXT NOT NULL,
  body            TEXT NOT NULL,
  created_at      INTEGER NOT NULL,
  delivered_at    INTEGER
);

CREATE INDEX idx_subagent_inbox_pending
  ON subagent_inbox_messages(task_id, delivered_at, created_at, id);
