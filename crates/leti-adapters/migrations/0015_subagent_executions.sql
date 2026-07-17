CREATE TABLE subagent_executions (
  task_id            TEXT PRIMARY KEY,
  root_session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  parent_session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  child_session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  agent_slug         TEXT NOT NULL,
  objective          TEXT NOT NULL,
  scope              TEXT,
  background         INTEGER NOT NULL DEFAULT 0,
  status             TEXT NOT NULL,
  terminal_reason    TEXT,
  output             TEXT NOT NULL DEFAULT '',
  cost_usd           TEXT,
  created_at         INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL,
  finished_at        INTEGER,
  version            INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_subagent_executions_root_status
  ON subagent_executions(root_session_id, status, updated_at DESC);
CREATE INDEX idx_subagent_executions_child_live
  ON subagent_executions(child_session_id, status);
