-- Adds opaque JSON `capabilities` column on sessions so callers can
-- declare interactive frontend affordances (e.g. ability to answer
-- `ask_user` prompts). Default '{}' so existing rows surface as the
-- `SessionCapabilities::default()` shape (every flag false) — matches
-- the headless-cloud safe-by-construction posture.

ALTER TABLE sessions
ADD COLUMN capabilities TEXT NOT NULL DEFAULT '{}'
  CHECK (json_valid(capabilities));
