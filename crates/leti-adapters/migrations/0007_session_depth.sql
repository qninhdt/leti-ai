-- Subagent nesting depth. Top-level sessions are depth=0; subagents
-- spawned via the in-process task tool inherit parent.depth + 1.
-- Bounded by LETI_SUBAGENT_MAX_DEPTH (default 3) at spawn time so a
-- runaway plan can't fan out unbounded.

ALTER TABLE sessions ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;
