-- Plan-mode support: track which agent PROFILE (slug) the session is
-- currently running and which slug it was on previously. Distinct from
-- `agent_id` (the UUID principal that owns the workspace) — the slug
-- selects an `AgentDefinition` from the registry, principal-id stays
-- stable across plan-mode toggles.
--
-- `current_agent_slug = NULL` is interpreted as the default "general"
-- profile by the runtime; existing rows therefore migrate without a
-- backfill. `previous_agent_slug = NULL` means the session has never
-- switched profiles yet.
--
-- Stored as plain TEXT — `AgentSlug` regex (kebab-case, 2-32 chars) is
-- enforced by the typed registry at switch time, not by SQLite. A
-- CHECK here would freeze the regex into the schema and break if the
-- rule loosens.

ALTER TABLE sessions
ADD COLUMN current_agent_slug TEXT;

ALTER TABLE sessions
ADD COLUMN previous_agent_slug TEXT;
