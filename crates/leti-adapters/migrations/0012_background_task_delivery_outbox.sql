CREATE TABLE background_task_delivery_outbox (
  parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  task_id           TEXT NOT NULL,
  child_session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  status            TEXT NOT NULL,
  output            TEXT NOT NULL,
  cost_usd          TEXT,
  scheduled_at      INTEGER,
  PRIMARY KEY(parent_session_id, task_id)
);

CREATE INDEX idx_background_task_delivery_outbox_pending
  ON background_task_delivery_outbox(scheduled_at)
  WHERE scheduled_at IS NULL;
