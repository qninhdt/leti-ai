-- Adds opaque JSON `extensions` column on sessions so integrators can
-- attach `user_id` (or any other JSON shape) without forking core types.
-- Default 'null' so existing rows surface as `Value::Null` in memory
-- (matches `SessionMeta::extensions` default); CHECK enforces JSON.
-- Core stays auth-blind — schema lives entirely in the integrator.

ALTER TABLE sessions
ADD COLUMN extensions TEXT NOT NULL DEFAULT 'null'
  CHECK (json_valid(extensions));
