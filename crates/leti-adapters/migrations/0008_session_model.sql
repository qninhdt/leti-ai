-- Per-session model override. NULL ⇒ the runtime falls back to
-- RuntimeConfig::default_model. When set, the session sends this model to
-- the provider and computes capabilities() for it so vision/quirk
-- detection matches the model actually used.

ALTER TABLE sessions ADD COLUMN model TEXT DEFAULT NULL;
