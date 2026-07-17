-- Phase 2 initial schema. Source of truth for queryable session/message
-- state. JSONL session log mirrors writes for replay/audit.

CREATE TABLE IF NOT EXISTS sessions (
  id                TEXT PRIMARY KEY,
  agent_id          TEXT NOT NULL,
  parent_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
  status            TEXT NOT NULL CHECK(status IN ('idle','running','cancelling','cancelled','errored')),
  permission_mode   TEXT NOT NULL CHECK(permission_mode IN ('read_only','workspace_write','danger')),
  version           TEXT NOT NULL DEFAULT '0.1.0',
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL,
  deleted_at        INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);

CREATE TABLE IF NOT EXISTS messages (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK(role IN ('system','user','assistant','tool')),
  seq         INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  meta        TEXT NOT NULL DEFAULT '{}',
  UNIQUE(session_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_messages_session_seq ON messages(session_id, seq);

CREATE TABLE IF NOT EXISTS parts (
  id          TEXT PRIMARY KEY,
  message_id  TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  kind        TEXT NOT NULL,
  payload     TEXT NOT NULL,
  UNIQUE(message_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_parts_message_seq ON parts(message_id, seq);

CREATE TABLE IF NOT EXISTS artifacts (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  key         TEXT NOT NULL,
  bytes_path  TEXT NOT NULL,
  size_bytes  INTEGER NOT NULL,
  mime        TEXT,
  created_at  INTEGER NOT NULL,
  UNIQUE(session_id, key)
);

CREATE TABLE IF NOT EXISTS events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  TEXT,
  kind        TEXT NOT NULL,
  payload     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_session_id ON events(session_id, id);

CREATE TABLE IF NOT EXISTS permission_decisions (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  ask_id      TEXT NOT NULL,
  permission  TEXT NOT NULL,
  decision    TEXT NOT NULL CHECK(decision IN ('allow','deny','always','never')),
  created_at  INTEGER NOT NULL,
  UNIQUE(session_id, ask_id)
);

-- §F: read history scoped per session for tool gating later phases.
CREATE TABLE IF NOT EXISTS session_reads (
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  path        TEXT NOT NULL,
  read_at     INTEGER NOT NULL,
  PRIMARY KEY(session_id, path)
);

CREATE INDEX IF NOT EXISTS idx_session_reads_session ON session_reads(session_id);
