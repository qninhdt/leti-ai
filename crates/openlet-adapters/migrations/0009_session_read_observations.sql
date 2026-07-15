-- Durable read-observation fingerprints for runtime reminders.
--
-- The pre-existing `session_reads` table recorded only (session_id, path,
-- read_at) — a path-only, in-effect boolean "was this read". The workspace-delta
-- reminder producer needs to detect that a previously-observed file CHANGED or
-- was DELETED since the agent last saw it, which requires a content fingerprint
-- and the read scope (a full read fingerprints the whole file; a range/partial
-- read must not claim the unseen remainder).
--
-- Additive columns with defaults so the existing rows and the existing
-- path-only `record_read` upsert keep working unchanged:
--   * fingerprint — content hash captured at observation time (NULL for legacy
--     rows and bare path-only records; a NULL fingerprint means "seen, content
--     unknown" and never reports a spurious change).
--   * scope       — 'full' | 'range' (default 'full'); a range read records that
--     only part of the file was observed.
ALTER TABLE session_reads ADD COLUMN fingerprint TEXT;
ALTER TABLE session_reads ADD COLUMN scope TEXT NOT NULL DEFAULT 'full';
