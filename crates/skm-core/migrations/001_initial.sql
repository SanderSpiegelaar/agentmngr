-- derived from library/ (rebuildable: `skm doctor --reindex`)
CREATE TABLE items (
  id            INTEGER PRIMARY KEY,
  kind          TEXT NOT NULL CHECK (kind IN ('skill','mcp','hook','subagent','plugin')),
  name          TEXT NOT NULL,
  description   TEXT NOT NULL DEFAULT '',
  rel_path      TEXT NOT NULL,              -- relative to library/
  stamp_mtime   INTEGER NOT NULL,           -- ns
  stamp_size    INTEGER NOT NULL,
  content_hash  TEXT,                       -- lazy
  meta_json     TEXT NOT NULL DEFAULT '{}', -- frontmatter / canonical fields
  problems_json TEXT NOT NULL DEFAULT '[]',
  source_json   TEXT,                       -- mirror of library.toml provenance
  indexed_at    INTEGER NOT NULL,
  UNIQUE (kind, name)
);

-- machine-local
CREATE TABLE scopes (
  id          INTEGER PRIMARY KEY,
  kind        TEXT NOT NULL CHECK (kind IN ('global','project')),
  path        TEXT UNIQUE,                  -- NULL for global
  name        TEXT NOT NULL,
  mode        TEXT NOT NULL DEFAULT 'copy' CHECK (mode IN ('copy','link')),
  auto_sync   INTEGER NOT NULL DEFAULT 1,   -- 0 = paused
  added_at    INTEGER NOT NULL
);
CREATE TABLE project_sets  (scope_id INTEGER REFERENCES scopes ON DELETE CASCADE, set_name TEXT, PRIMARY KEY (scope_id, set_name));
CREATE TABLE project_items (                 -- direct (non-set) project items
  scope_id  INTEGER REFERENCES scopes ON DELETE CASCADE,
  item_kind TEXT NOT NULL CHECK (item_kind IN ('skill','mcp','hook','subagent')),
  item_name TEXT NOT NULL,
  agents_json TEXT,                           -- NULL = '*' / inherit from set; ignored for skills
  PRIMARY KEY (scope_id, item_kind, item_name)
);

CREATE TABLE agents (
  id         TEXT PRIMARY KEY,              -- registry id, e.g. 'claude-code'
  detected   INTEGER NOT NULL,
  enabled    INTEGER NOT NULL,
  version    TEXT,
  detected_at INTEGER
);

-- ownership ledger: everything we wrote
CREATE TABLE ledger (
  id          INTEGER PRIMARY KEY,
  scope_id    INTEGER NOT NULL REFERENCES scopes,
  agent_id    TEXT,                          -- NULL for .agents/skills targets
  item_kind   TEXT NOT NULL,
  item_name   TEXT NOT NULL,
  target      TEXT NOT NULL,                 -- absolute path
  entry_key   TEXT NOT NULL DEFAULT '',      -- e.g. 'mcpServers.github' or '' for whole path
  how         TEXT NOT NULL CHECK (how IN ('link','copy','entry','file','cli','lock','exclude')),
  written_hash TEXT NOT NULL,
  written_at  INTEGER NOT NULL,
  UNIQUE (target, entry_key)
);

CREATE TABLE updates (
  item_name TEXT PRIMARY KEY, current_rev TEXT, latest_rev TEXT,
  checked_at INTEGER, state TEXT CHECK (state IN ('none','available','ignored','error')), error TEXT
);

CREATE TABLE foreign_entries (               -- discovered, not ours
  target TEXT PRIMARY KEY, scope_id INTEGER, kind TEXT, first_seen INTEGER, hint_json TEXT
);

CREATE TABLE ops (                            -- activity log + undo
  id INTEGER PRIMARY KEY, at INTEGER NOT NULL, actor TEXT NOT NULL, summary TEXT NOT NULL,
  plan_json TEXT NOT NULL, inverse_json TEXT, result TEXT NOT NULL
);
