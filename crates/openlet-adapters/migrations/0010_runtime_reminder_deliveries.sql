-- Durable idempotency keys for model-visible, UI-hidden runtime reminders.
-- The associated message and part ids make provenance structural: callers
-- never infer trusted control input from text content.
CREATE TABLE runtime_reminder_deliveries (
  session_id       TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  reminder_kind    TEXT NOT NULL,
  stable_key       TEXT NOT NULL,
  projection_epoch INTEGER NOT NULL,
  message_id       TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_id          TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  PRIMARY KEY(session_id, reminder_kind, stable_key, projection_epoch),
  UNIQUE(part_id)
);

CREATE INDEX idx_runtime_reminder_deliveries_message
  ON runtime_reminder_deliveries(message_id);
