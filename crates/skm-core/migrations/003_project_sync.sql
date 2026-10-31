CREATE TABLE project_pending (
  scope_id INTEGER NOT NULL REFERENCES scopes ON DELETE CASCADE,
  change_key TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (scope_id, change_key)
);
