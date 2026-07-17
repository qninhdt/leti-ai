-- `scheduled_at` meant "enqueued", not "processed". A process crash after
-- enqueue could therefore permanently strand a background completion. Keep
-- the legacy column for migration compatibility, but use a lease lifecycle
-- for all new reads: pending -> leased -> delivered. Existing `scheduled_at`
-- rows are conservatively replayed: the old schema cannot prove whether the
-- in-memory parent turn completed before a process crash.
ALTER TABLE background_task_delivery_outbox
  ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE background_task_delivery_outbox
  ADD COLUMN lease_id TEXT;
ALTER TABLE background_task_delivery_outbox
  ADD COLUMN lease_expires_at INTEGER;
ALTER TABLE background_task_delivery_outbox
  ADD COLUMN delivered_at INTEGER;
ALTER TABLE background_task_delivery_outbox
  ADD COLUMN delivery_attempts INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_background_task_delivery_outbox_claimable
  ON background_task_delivery_outbox(delivery_state, lease_expires_at);
