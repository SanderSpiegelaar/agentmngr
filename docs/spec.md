# Technical spec: skillsmgr

Status: draft v3 (all open questions answered, 31 Oct 2026 19:15) · 31 Oct 2026 (Europe/Amsterdam) · Inputs: `research.md`, `../skills-manager-analysis.md`

Names (confirmed by Sander):
- app: **skillsmgr**;
- CLI binary: **`skm`**;
- data dir: **`~/.skillsmgr`**;
- env override: **`SKILLSMGR_HOME`**.

---

## 1. Principles

1. **Agents read files; we write the fewest files possible.** Skills go to one place: `.agents/skills` (global `~/.agents/skills`, project `<repo>/.agents/skills`). Other resource types go to each agent's native config only because no shared location exists yet.
2. **Desired state → plan → apply.** Every change, from the GUI, CLI or sync, is computed as a plan of file actions, checked against what's on disk, then applied atomically. A dry-run is the same plan without the apply step.
3. **Never touch what we don't own.** An ownership ledger records every file, link or config entry we wrote, with the hash we wrote. Anything else is *foreign*: we show it, offer to adopt it, and never delete or overwrite it silently.
4. **Startup never scans.** The window paints from the SQLite index. Verification happens in the background, scoped to what changed.
5. **One owner per fact.** Synced definitions live in plain files in the library, which is git-friendly. SQLite holds the derived index plus machine-local state. Nothing is authoritative in two places.
6. **KISS/YAGNI.** System `git` instead of a git library. Agents are described as data, not code. No plugin system, no daemon, no server in v1.

## 2. Concepts (the only nouns in the UI)

| Term | Meaning |
|---|---|
| **Item** | A managed thing of one **kind**: `skill`, `mcp`, `hook`, `subagent` or `plugin`. |
| **Library** | All items you own, stored under `~/.skillsmgr/library/`. Owning an item doesn't make it active. |
| **Scope** | Where something is active: **Global** (`~` for all agents) or a **Project** (a registered repo folder). |
| **Set** | A named bundle of skills, MCP servers, hooks and subagents (e.g. "Rust", "TypeScript"). A project can **use** sets. This is a live link: edit the set and every project using it updates. Plugins are not in sets, because they are Global-only in v1. |
| **Agent** | A coding tool (Claude Code, Codex, Cursor, …) described in the registry. Agents are *detected* (installed) and *enabled* (we manage them). |
| **Source** | Where an item came from: a git repo plus optional ref and subpath, skills.sh (which resolves to git), or a local folder. |

States a user can see per item and scope: **On**, **Off**, **Foreign** (exists on disk, not ours), **Conflict** (desired but blocked by a foreign file), **Update available** and **Edited** (local changes since install).

Words that don't appear in the UI: preset, deploy, sync-mode, adapter.

## 3. Decision record (summary)

| # | Decision | Status |
|---|---|---|
| D1 | Rust + GPUI via **`gpui-kit` (exact pin)**; macOS first; the core crate has no GPUI dependency | Sander |
| D2 | Skills target = `.agents/skills` only. **No bridge or workaround for agents that can't read it.** Claude Code won't see managed skills until it supports `.agents/skills` (anthropics/claude-code#31005, #16345) | Sander |
| D3 | Library content lives in `~/.skillsmgr/library/skills/<name>`. `~/.agents/skills/<name>` is a symlink to it when the skill is on globally. There is no alternative placement mode. | Sander |
| D4 | SQLite (rusqlite, bundled, WAL) = index + machine-local state only. Synced definitions = files (`library.toml`). | Sander (final) |
| D5 | System `git` CLI for all git operations; system git (Xcode CLT) is required, and `skm doctor` guides the install | Sander (final) |
| D6 | MCP, hooks and subagents: canonical file per item, rendered per agent, with surgical ownership-tracked edits, at **Global and Project scope in v1** | Sander |
| D7 | Plugins (v1): list, enable/disable and install for Claude Code and Codex via an app-hosted local marketplace, driving the agents' own CLIs; Global scope only; not in sets. Cursor plugins are listed read-only | Sander (final) |
| D8 | Sets can contain skills, MCP servers, hooks and subagents | Sander (final); see §6.2 |
| D9 | skills.sh = search only (unauthenticated endpoint); install always via git; no leaderboard, no outreach | Sander |
| D10 | Default project mode = **committed copies** + npx-compatible `skills-lock.json`. Link mode (git-excluded symlinks) is optional | Sander |
| D11 | Foreign skills in `~/.agents/skills` (npx skills, APM, hand-made) are adopted **only on an explicit Adopt**, never automatically | Sander (final) |
| D12 | Global MCP secrets are stored in the macOS Keychain and written as real values into user-level agent configs; the library and git keep only `${secret:…}` placeholders. Project files get env references only | Sander (final) |
| D13 | In copy mode, `skills-lock.json` is merged: entries the app doesn't own (e.g. written by npx skills) are kept byte-for-byte untouched; only our entries are added, updated or removed | Sander (final) |

## 4. Architecture

```
┌───────────────┐   ┌───────────────┐
│ skm (CLI)     │   │ skillsmgr.app │  GPUI (gpui-kit), views + view-models
└──────┬────────┘   └──────┬────────┘
       │ calls             │ calls + subscribes (core events → entity updates)
┌──────▼───────────────────▼────────────────────────────────────────┐
│ skm-core                                                          │
│  paths/config · registry · store(SQLite) · index · watcher        │
│  engine(plan/apply, ownership ledger, trash) · render(per format) │
│  sources(git, skills.sh, local, npx-lock) · updates(diff/merge)   │
│  sync(git) · secrets(keychain) · errors · events                  │
└───────────────────────────────────────────────────────────────────┘
        │ std::process::Command → git, claude, codex  │ fs  │ HTTPS (ureq) → skills.sh
```

- **Threads.** The core owns:
  - a **store thread** (the single SQLite writer, with a command channel);
  - a **worker pool** (index/hash/render jobs; `rayon` or a small thread pool);
  - a **watcher thread**.

  Network and git calls run on worker threads with timeouts. The GUI never blocks: it sends commands and receives `CoreEvent`s (`ItemsChanged{ids}`, `PlanReady`, `ApplyDone`, `SyncState`, `UpdateFound`, `Error`) through a channel bridged into GPUI entities.
- **App and CLI concurrency.** Both use `skm-core`. Apply phases take an inter-process advisory lock at `$SKILLSMGR_HOME/lock` (`fd-lock`), held only during apply, never during network I/O. After every apply, the CLI writes an `ops` row and touches `$SKILLSMGR_HOME/.changed`, which the app's watcher picks up.

### 4.1 Crate layout (cargo workspace)

```
skillsmgr/
  Cargo.toml                  # workspace; edition 2024; MSRV pinned
  crates/
    skm-core/                 # no GPUI, no tokio
      src/{paths,config,registry,store,index,watcher,engine,render,
           sources,updates,sync,secrets,error,events,model}.rs (modules → dirs as they grow)
      registry/agents.toml    # embedded agent registry (data)
      migrations/*.sql
    skm-cli/                  # clap; binary `skm`; human + --json output
    skm-app/                  # GPUI app; binary `skillsmgr`
      src/{app,theme,matrix,detail,discover,projects,sets,updates,sync,settings,palette}.rs
  tests/fixtures/             # fake HOME trees per agent; golden render outputs
```

Three crates is enough. Split `skm-core` only when compile times demand it (YAGNI).

## 5. On-disk layout

### 5.1 Data dir: `$SKILLSMGR_HOME` (default `~/.skillsmgr`)

Resolution order:
1. `--home` CLI flag;
2. `SKILLSMGR_HOME`;
3. `~/.skillsmgr`.

The same rule applies to app and CLI, and the resolved path is shown in Settings and in `skm doctor`. (We deliberately don't use `~/Library/Application Support` on macOS. Sander wants a findable, portable, home-dir folder.)

```
~/.skillsmgr/
  config.toml            # machine-local prefs (theme, agent path overrides, sync remote, update interval)
  state.db               # SQLite: index + machine-local state (WAL: state.db-wal, -shm)
  lock                   # inter-process apply lock
  .changed               # touched after every apply (app watches it)
  library/               # ← everything here is synced (it is the git repo when sync is on)
    library.toml         # provenance, sets, global desired state (schema below)
    skills/<name>/       # skill content (SKILL.md, scripts/, …), byte-for-byte
    mcp/<name>.json      # canonical MCP server entry
    hooks/<name>.json    # canonical hook (Claude/Codex schema)
    subagents/<name>.md  # canonical subagent (Claude-style frontmatter)
    plugins/             # local marketplace
      .claude-plugin/marketplace.json
      .agents/plugins/marketplace.json
      <plugin-name>/     # Agent Plugins layout (plugin.json, skills/, mcp.json …)
    .gitignore
  cache/
    git/<host>/<owner>/<repo>.git    # bare, blobless, shallow mirrors for sources
    base/<skill>/<hash>/             # pristine installed snapshot (3-way base)
    skillssh/                        # search response cache (TTL 10 min)
  trash/<YYYYMMDD-HHMMSS>-<op>/      # everything we remove or replace; auto-purged after 30 days
  logs/skillsmgr.log (rolling)
```

### 5.2 `library.toml`

This file is human-readable, sorted and has one table per item, so git merges stay clean.

```toml
schema = 1

[global]
skills   = ["code-review", "git-commit"]          # on in ~/.agents/skills
# per-agent enablement for non-skill kinds (Global scope)
mcp      = { github = ["claude-code", "codex", "cursor"] }
hooks    = { "fmt-on-edit" = ["claude-code", "codex"] }
subagents = { reviewer = ["claude-code", "codex", "gemini"] }
plugins  = { "acme-tools" = ["claude-code", "codex"] }

[sets.rust]
description = "Rust projects"
skills    = ["cargo-check", "rust-api-guidelines"]
mcp       = { "rust-docs" = ["*"] }          # "*" = every enabled agent that supports the kind
hooks     = { "cargo-fmt-on-edit" = ["claude-code", "codex"] }
subagents = { "rust-reviewer" = ["*"] }

[skills.code-review]
source = { kind = "git", url = "https://github.com/acme/skills", ref = "main", path = "skills/code-review" }
rev = "3f2a…"            # commit installed
base_hash = "sha256:…"   # computedHash of pristine install (npx-compatible algorithm)
installed_at = 2026-10-31T18:00:00+01:00

[skills.my-local-skill]
source = { kind = "local", origin = "/Users/sander/dev/my-skill" }   # origin is informational only
```

Machine-local data is NOT in this file: project registrations, project→set links, direct project items, detected agents and the ledger. Those live in SQLite (§9), because paths differ per machine.

### 5.3 Project footprint

Rule: **we create nothing in a repo until the user turns on an item or set for that project.** Then we touch only the target paths for that project in the registry (§10):
- `<repo>/.agents/skills/<name>`: one entry per active skill;
- `<repo>/skills-lock.json`: in copy mode;
- project config files for MCP, hooks and subagents (e.g. `.mcp.json`, `.claude/settings.json`, `.codex/config.toml`, `.cursor/mcp.json`, `.claude/agents/<name>.md`; full table in §11.5), with entry-level ownership;
- `.git/info/exclude`: in link mode only.

The app **never runs `git add` or `git commit` in a project repo.** Changes show up as normal working-tree changes for the user to commit.

Project mode (per project, applies to all kinds):
- **copy** (default, "committed"):
  - skills are real copies in `<repo>/.agents/skills/<name>`, and the app writes an npx-compatible `skills-lock.json` (v1: sorted keys, `computedHash`, relative local sources). The lock is merged, not replaced: entries we don't own stay untouched (D13). Teammates without the app restore with `npx skills experimental_install`;
  - MCP, hooks and subagents are written into the agents' shared project files, which the team commits.
- **link** (optional, "private to this machine"):
  - skills are symlinks to the library, added to `.git/info/exclude`;
  - project config files we *create* are also added to `.git/info/exclude`. Files already tracked by git are edited in place, and the plan preview says so;
  - no `skills-lock.json` is written.

Switching the mode is a normal plan (copies ↔ links, lock written/removed) with a preview.

## 6. Skills

### 6.1 Global: library + links (D3)

- Content lives in `library/skills/<name>/`.
- On globally means `~/.agents/skills/<name>` is a symlink to `$SKILLSMGR_HOME/library/skills/<name>`.
- Off globally means the link is removed. Content never moves.

Why: there are no moves on toggle (the upstream data-loss pattern), project-only skills have a home agents don't read globally, and git sync has a single root. Every agent that reads `.agents/skills` follows symlinks.

**Foreign entries in `~/.agents/skills`** (installed by npx skills, APM or by hand) are listed as Foreign with an **Adopt** action. Adopt copies the directory into the library, verifies the hash, moves the original into trash, and replaces it with our link. Provenance is pre-filled from `~/.agents/.skill-lock.json` when the skill appears there. Nothing happens to foreign entries without a click.

**Claude Code:** it reads only `~/.claude/skills` and `.claude/skills`, so it does not see managed skills. The matrix shows its skills column as "— not supported (reads only ~/.claude/skills)" with links to anthropics/claude-code#31005 and #16345. The app never writes to `~/.claude/skills` or `.claude/skills`. Its MCP, hooks, subagents and plugins are fully managed.

### 6.2 Project scope and sets

Effective project items, per kind:

  effective(kind) = ⋃(items of that kind in the sets the project uses) ∪ (direct project items) − (items already on globally for the same agent)

Global items are subtracted to avoid duplicates: Codex and Cursor would list a skill twice, and agents would show a server twice or let the project entry shadow the global one. For non-skill kinds, a set entry lists agents explicitly or `"*"` ("every enabled agent that supports this kind"). A direct project item can override the agent list.

**Sets are a live link (auto-sync).** Editing a set or updating a library item computes a plan for every project using it, which is applied automatically:
- copy mode: rewrite the committed copies in `<repo>/.agents/skills/<name>` and regenerate `skills-lock.json`. Upsert or remove our entries in the project config files and rewrite rendered subagent files;
- link mode: add or remove links (content changes need no action) and update config entries the same way.

Safety rules for auto-sync:
1. A project copy or entry whose hash differs from the ledger (edited in the repo, e.g. by a teammate) is **not** overwritten. It shows as **Edited**, with actions "pull into library", "overwrite" or "keep".
2. Auto-sync only changes the working tree. It never stages or commits.
3. Each project has a "pause auto-sync" toggle. Paused projects show "n pending changes".
4. Every auto-applied plan appears in Activity with Undo.

**Re-attach on another machine or for a teammate's checkout.** When a project is registered and its `<repo>/.agents/skills/<name>` or config entry content-hash-matches a library item, it is recorded in the ledger as ours (no Foreign noise). Mismatches stay Foreign or Edited. With the app, `skills-lock.json` lets the project's sets be suggested.

### 6.3 Validation and indexing

- Parse only the frontmatter (stop reading at the second `---`, with a cap of 64 KB).
- Warnings:
  - name/dir mismatch;
  - name chars;
  - description length;
  - missing SKILL.md;
  - files > 5 MB (sync warning);
  - symlinks pointing outside the skill.
- `content_hash` is computed lazily (before update or sync, or on demand) with the npx `computedHash` algorithm, skipping `.git`.

## 7. Engine: ownership, plans, apply

### 7.1 Target classification (kept from upstream)

For every target path, classify it as one of:
- `Absent`;
- `OurLink(item)` (a symlink whose target resolves into our library);
- `ForeignLink(target)`;
- `RealDir`;
- `RealFile`.

For config entries, classify it as one of:
- `Absent`;
- `Ours(hash matches ledger)`;
- `OursModified(hash differs)`;
- `Foreign`.

### 7.2 Ledger rule

We may modify or remove a target only if it is `OurLink`, or `Ours`, or `OursModified` *after explicit user approval*. Everything else produces a **PlanIssue** (shown in the UI) and no action. Replacing a foreign target needs a deliberate user action ("Replace — original goes to Trash").

### 7.3 Shared target files

Some project and user files are read by more than one agent:
- `.mcp.json`: Claude Code + Copilot CLI;
- `.claude/agents`: Claude Code + Cursor;
- `.codex/agents`: Codex + Cursor.

The registry records this as `also_read_by`. The planner:
- (a) never writes the same item twice into files that one agent reads both of;
- (b) reports the derived visibility in the matrix ("via .mcp.json", "via Claude");
- (c) for Copilot, writes project MCP to `.github/mcp.json` rather than `.mcp.json`, so Claude and Copilot dialects never share an entry.

### 7.4 Plan and apply

- `Action` = one of:
  - `Link{target, to}`;
  - `Unlink{target}`;
  - `CopyTree{from, to}`;
  - `UpsertEntry{file, format, key, value}`;
  - `RemoveEntry{file, format, key}`;
  - `WriteFile{path, bytes}`;
  - `RunCli{agent, argv}`;
  - `Trash{path}`.
- `Plan { actions, issues, summary }`. Plans are **scoped**: they are computed only for the items and scopes affected by a change, so most plans take microseconds.
- Apply:
  1. take the lock;
  2. re-classify each target (abort that action if it changed since the plan);
  3. execute;
  4. write the ledger row;
  5. append an `ops` row with the inverse actions (for **Undo**);
  6. release the lock;
  7. emit `ItemsChanged`.
- Atomicity:
  - files: write `*.skm-tmp` in the same dir, `fsync`, then `rename`;
  - links: create `*.skm-tmp` link, then `rename`;
  - config edits: read → edit in memory → hash-check that the file is unchanged since read → atomic write → backup copy of the previous file into trash.
- Undo: the last 50 ops can be undone from the Activity view or with `skm undo`.

## 8. Watcher and indexing

Startup sequence (budgets in §14):
1. Resolve paths and open the DB.
2. Load `items` and `deployments` from SQLite → build view models.
3. **First paint.**
4. In the background: start the watcher, then run a **stamp catch-up**: `stat` each library item dir and its SKILL.md (mtime + size, about 2 syscalls per item), each ledger target, and each watched config file. Re-index only mismatches. Detect agents (stat the registry's `detect` paths).
5. Update check 5 min after launch (if enabled), then every 6 h.

Watched paths (notify 8 + notify-debouncer-full, debounce 250 ms):
- `library/` (recursive; ignore `.git/`, `node_modules/`, `target/`, `.venv/`, `__pycache__/`, `.DS_Store`);
- `~/.agents/skills` (depth 1 matters; deeper events are mapped to their top-level entry);
- for registered projects: `<project>/.agents/skills` (and `<project>/.agents` creation), `skills-lock.json`, and each project config file or agents dir in the registry's project paths (watched via its parent dir, non-recursive, filtered by name). Projects are watched only while registered;
- agent config files, by watching the parent dir non-recursively and filtering by file name (e.g. `~/.claude/settings.json`, `~/.codex/config.toml`, `~/.cursor/mcp.json`, `~/.claude.json`, whose events compare only our entries' hashes);
- `$SKILLSMGR_HOME/.changed`.

Event pipeline: path → item or target lookup (an in-memory prefix map) → re-index that item / re-classify that target → diff against the previous row → `ItemsChanged{ids}` → the GUI updates those rows only. An FSEvents "must rescan" flag rescans just that directory. **No polling, ever.**

## 9. SQLite schema (machine-local; derived parts rebuildable)

```sql
PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;  -- user_version = schema version

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
```

## 10. Agent registry (data, not code)

`skm-core/registry/agents.toml` is embedded. Users can override paths per agent in `config.toml`. Every entry is validated at load time.

```toml
[agents.claude-code]
name = "Claude Code"
detect = ["~/.claude", "bin:claude"]
config_dir_env = "CLAUDE_CONFIG_DIR"
skills = { reads_universal = false, note = "reads only ~/.claude/skills (anthropics/claude-code#31005, #16345)" }
mcp = { dialect = "claude",
        user    = { file = "~/.claude.json", format = "json", key = "mcpServers", write_via = "cli" },
        project = { file = ".mcp.json", format = "json", key = "mcpServers", also_read_by = ["copilot"],
                    trust = "Claude asks to approve project servers on first use" } }
hooks = { dialect = "claude",
          user    = { file = "~/.claude/settings.json", key = "hooks" },
          project = { file = ".claude/settings.json", key = "hooks" } }
subagents = { format = "md-claude", user = "~/.claude/agents", project = ".claude/agents", also_read_by = ["cursor"] }
plugins = { driver = "claude-cli" }

[agents.codex]
name = "Codex"
detect = ["~/.codex", "bin:codex"]
skills = { reads_universal = true }
mcp = { dialect = "codex",
        user    = { file = "~/.codex/config.toml", format = "toml", key = "mcp_servers" },
        project = { file = ".codex/config.toml", format = "toml", key = "mcp_servers", trust = "loaded only for trusted projects" } }
hooks = { dialect = "claude", requires = "features.hooks",
          user    = { file = "~/.codex/hooks.json", key = "hooks" },
          project = { file = ".codex/hooks.json", key = "hooks", trust = "trusted projects + per-hook trust" } }
subagents = { format = "toml-codex", user = "~/.codex/agents", project = ".codex/agents", also_read_by = ["cursor"] }
plugins = { driver = "codex-cli" }
# … cursor, gemini, opencode, copilot (v1; project paths in §11.5); windsurf, antigravity, kiro, … (later, data-only additions)
```

v1 agents with full support: **Claude Code, Codex, Cursor, Gemini CLI, OpenCode, GitHub Copilot CLI.** Claude Code is managed for MCP, hooks, subagents and plugins, but not skills (§6.1). Any other `.agents/skills` reader (Windsurf, Cline, Warp, Zed, …) gets skills for free and appears in the matrix as "reads .agents/skills" with no per-kind config until a registry entry is added.

## 11. Per-kind handling

### 11.1 MCP servers

- Canonical (`library/mcp/<name>.json`):
  ```json
  {"type":"stdio","command":"npx","args":["-y","@acme/mcp"],"env":{"API_KEY":"${secret:API_KEY}"}}
  ```
  or
  ```json
  {"type":"http","url":"https://…","headers":{"Authorization":"Bearer ${secret:TOKEN}"}}
  ```
- **Renderers** (`render::mcp::<dialect>`):
  - claude → JSON `mcpServers` entry;
  - codex → `toml_edit` `[mcp_servers.<name>]` (`bearer_token_env_var` used when possible);
  - cursor → JSON;
  - gemini → JSON (`httpUrl` for HTTP; names with `_` are rejected with a hint);
  - opencode → `mcp.<name>` with `type: local|remote`, `command` array, `environment`;
  - copilot → JSON `type: local|http`.

  Unsupported fields are dropped per dialect and listed in the item detail.
- **Writes:** surgical edit of exactly one key path. Unknown keys, order and comments (TOML/JSONC) are preserved. `~/.claude.json` is written via `claude mcp add-json --scope user <name> '<json>'` and `claude mcp remove --scope user <name>` when the CLI is present; otherwise a hash-checked direct edit is made with a "quit Claude Code first" warning.
- **Secrets (Global scope):** `${secret:NAME}` placeholders resolve from the macOS Keychain (service `skillsmgr`, account `<server>/<NAME>`) at render time. Resolved values are written into the user-level agent config, because agents need literal values and these files are outside any repo. The ledger stores the hash of the rendered entry, while the library and git keep only the placeholder. A new device asks for missing secrets after sync.
- **Project scope secrets (never literal):** project files are committed in the default mode, so at project scope `${secret:NAME}` renders as an **env reference** in the dialect's syntax:
  - claude `${NAME}`;
  - cursor `${env:NAME}`;
  - codex `env_vars = ["NAME"]` / `bearer_token_env_var`;
  - gemini `${NAME}`;
  - opencode `{env:NAME}`.

  Dialects with no documented env syntax (Copilot CLI at time of writing) refuse with `E_SECRET_IN_PROJECT` and a hint. The user supplies the env var. The item detail shows which vars a project needs.
- **Import:** scan each agent's config (user and registered-project files) for existing servers → Foreign entries → **Adopt** creates a canonical file (detected secrets are moved to the Keychain, with confirmation).

### 11.2 Hooks

- Canonical (`library/hooks/<name>.json`): `{ "event": "PostToolUse", "matcher": "Edit|Write", "command": "…", "timeout": 30 }`. This is a single hook in Claude/Codex terms; the name is ours.
- An event map in the registry (excerpt):

  | canonical | claude | codex | cursor | gemini | copilot |
  |---|---|---|---|---|---|
  | PreToolUse | PreToolUse | PreToolUse | preToolUse | BeforeTool | preToolUse |
  | PostToolUse | PostToolUse | PostToolUse | postToolUse | AfterTool | postToolUse |
  | SessionStart | SessionStart | SessionStart | sessionStart | SessionStart | sessionStart |
  | Stop | Stop | Stop | stop | — | sessionEnd |
  | UserPromptSubmit | UserPromptSubmit | UserPromptSubmit | beforeSubmitPrompt | BeforeAgent | userPromptSubmitted |

  The full table is filled during T-066 from the docs cited in research §5.2. Unmapped → the cell shows "not supported".
- Writes:
  - Claude, Gemini: entry inside settings.json `hooks.<Event>[]`. Hook groups have no IDs, so ownership = exact match of the JSON object we wrote (hash in ledger).
  - Codex: same, in `~/.codex/hooks.json`, plus a check (and offer) for `features.hooks = true`.
  - Cursor: `~/.cursor/hooks.json` (`version: 1`).
  - **Copilot: our own file `~/.copilot/hooks/skillsmgr.json`** (project: `.github/hooks/skillsmgr.json`). Copilot allows separate files, which makes this the simplest case.
- Safety: enabling a hook shows the exact command and the agents it goes to. Hooks from remote sources are flagged "runs code".

### 11.3 Subagents

- Canonical: `library/subagents/<name>.md` (Claude frontmatter superset).
- Rendered **copies**, not links, because formats differ per agent:
  - Claude `~/.claude/agents/<name>.md`;
  - Codex `~/.codex/agents/<name>.toml` (`name`, `description`, `developer_instructions` = body, `model` if mappable);
  - Gemini `~/.gemini/agents/<name>.md` (allowed keys only);
  - OpenCode `~/.config/opencode/agents/<name>.md`;
  - Copilot `~/.copilot/agents/<name>.agent.md`.
- **Cursor**: since it reads `.claude/agents` and `.codex/agents`, we write `~/.cursor/agents` only if neither Claude nor Codex is enabled. The matrix shows Cursor as "via Claude".
- Ownership = file-level ledger hash. An edited rendered copy shows as **Edited**; the user picks "keep theirs → pull into library" or "overwrite".

### 11.4 Plugins

- **Inventory:**
  - Claude: read `~/.claude/plugins/installed_plugins.json` and `enabledPlugins` in settings;
  - Codex: `[plugins.*]` in config.toml and the plugin cache;
  - Cursor: installed plugins dir (read-only in v1).
- **Install:** `skm add <git-url> --kind plugin` copies the plugin into `library/plugins/<name>/` and regenerates both marketplace manifests. On first use, the local marketplace is registered via the agent CLIs:
  - `claude plugin marketplace add <library>/plugins`;
  - `codex plugin marketplace add <library>/plugins`.
- **Enable/disable** via `claude plugin install|enable|disable <name>@skillsmgr` and `codex plugin …` (or `[plugins."<name>@skillsmgr"] enabled` via toml_edit if the CLI lacks the verb).
- Plugins installed from other marketplaces are shown read-only with enable/disable only.
- Plugins are Global scope only in v1 and are not part of sets.
- Non-goals for v1: converting between plugin formats; Gemini extensions; OpenCode JS plugins (shown as "not supported").

### 11.5 Project scope for MCP, hooks and subagents (v1)

Same canonical items and renderers as Global; only the target paths differ (registry `project` entries; evidence in research §5.5).

| Agent | MCP (project) | Hooks (project) | Subagents (project) | Trust hint shown |
|---|---|---|---|---|
| Claude Code | `.mcp.json` `mcpServers` | `.claude/settings.json` `hooks` | `.claude/agents/<n>.md` | approve project servers in Claude |
| Codex | `.codex/config.toml` `[mcp_servers.<n>]` | `.codex/hooks.json` | `.codex/agents/<n>.toml` | trust the project in Codex; review hooks |
| Cursor | `.cursor/mcp.json` | `.cursor/hooks.json` | via `.claude/agents` / `.codex/agents`, else `.cursor/agents/<n>.md` | — |
| Gemini CLI | `.gemini/settings.json` `mcpServers` | `.gemini/settings.json` `hooks` | `.gemini/agents/<n>.md` | trusted folders |
| OpenCode | `opencode.json(c)` `mcp.<n>` (v1) / `mcp.servers.<n>` (v2, detected) | — not supported | `.opencode/agents/<n>.md` | — |
| Copilot CLI | `.github/mcp.json` `mcpServers` (also sees `.mcp.json`) | `.github/hooks/skillsmgr.json` | `.github/agents/<n>.agent.md` | trust the folder in Copilot |

Rules:
1. Entry-level ownership exactly as at Global scope (ledger key = file + entry path).
2. Secrets are env references only (§11.1).
3. Shared files are handled per §7.3.
4. A trust hint appears once per project and agent. We never modify an agent's trust store.
5. Project hook commands typically reference repo-relative scripts. The canonical hook may use `${PROJECT_ROOT}`, which renders to each dialect's project-dir variable where one exists (e.g. Claude `$CLAUDE_PROJECT_DIR`) and otherwise to a relative path.
6. Unparsable project config file → no write, with an issue on that project.

## 12. Sources, install, updates

### 12.1 Source parsing (`skm add <spec>`)

| Spec | Meaning |
|---|---|
| `owner/repo` | github.com repo; discover all skills |
| `owner/repo@skill` | a single skill by name (skills.sh convention) |
| `https://github.com/o/r/tree/<ref>/<path>` | ref + subpath |
| any git URL (`https://…`, `git@…:…`), optional `#ref` and `--path` | generic git |
| `./path` or `/abs/path` | local folder import (copied; origin remembered) |
| `skills.sh/<owner>/<repo>/<skill>` | resolved to `owner/repo@skill` |

### 12.2 Install flow

1. Resolve to (url, ref, path).
2. `git clone --bare --filter=blob:none --depth 1` (or `git fetch` if the mirror is cached) into `cache/git/…`.
3. List `SKILL.md` paths via `git ls-tree -r --name-only`.
4. Preview: name, description, files, size, and whether it contains scripts or hooks.
5. User picks.
6. `git archive <rev> <path>` → extract to temp → validate → rename into `library/skills/<name>`.
7. Write provenance + `base_hash` into library.toml; snapshot to `cache/base/`.
8. Optional: turn on now (Global, a project or a set). After a library update, projects in copy mode get rewritten copies and a regenerated lock via auto-sync (§6.2).

Name collisions prompt for **rename** (which also updates the frontmatter `name`) or **replace** (old version goes to trash).

### 12.3 skills.sh

- `GET https://skills.sh/api/search?q=&limit=50` (cached 10 min). Base URL is overridable: `SKILLSMGR_SKILLS_API` (falls back to `SKILLS_API_URL`).
- The Discover view shows name, source, installs and an "Add" button that resolves to `owner/repo@skill`.
- No leaderboard in v1 (research §4).

### 12.4 Updates (notify → diff → approve)

- **Check:** group items by source repo, so each repo is fetched once. Run `git ls-remote <url> <ref>`; if the commit changed, run `git fetch --depth 1 --filter=blob:none`, then compare `git rev-parse <new>:<path>` with the installed tree. Only real subpath changes count as updates. The check runs on a schedule (6 h) and manually, and never blocks the UI.
- **Notify:** a badge on the row, a count in the sidebar, and an optional macOS notification.
- **Review:** a three-panel diff per file:
  - upstream changes (base → theirs);
  - your local edits (base → yours);
  - the merged result.

  `Edited` is detected via `content_hash != base_hash`.
- **Apply** (only after approval), per file:
  - unchanged locally → take theirs;
  - changed only locally → keep yours;
  - changed on both sides → `git merge-file` 3-way. A conflict needs a choice per file: yours, theirs, or edit.

  Files added locally are always kept; files deleted upstream are removed only if they're unchanged locally. The previous version goes to trash, then library.toml `rev`/`base_hash` and `cache/base` are updated.
- **Ignore this version** is available per item.

## 13. Git sync (multi-device backup)

- The repo is `library/`, and sync is opt-in. `skm sync init [--remote <url>]` runs `git init` in library/ and writes a `.gitignore` covering OS junk.
  - Remote empty: push.
  - Remote has content and the local library is empty: clone.
  - Both have content: fetch + merge, with the conflict flow below. Before merging, the whole library/ is backed up to `trash/`.
- **Auto-sync loop** (app running, or `skm sync`):
  1. After any library change, debounce 30 s, then `git add -A` and `git commit -m "skm: <summary>"`.
  2. Every 10 min and at startup: `git fetch`. If the remote moved, `git merge --no-edit` (no rebase), then push.
  3. If merged changes touched desired state, compute a plan → apply (same safety rules). Changes that would remove foreign-blocked targets become issues, never forced actions.
- **Conflicts:** stop auto-sync and show a banner. A per-file resolver offers mine / theirs / open in $EDITOR. `library.toml` is laid out one table per item, so real conflicts should be rare.
- Limits:
  - files > 50 MB block the commit with a clear message;
  - files > 5 MB warn;
  - secrets are never committed (placeholders only).

  `doctor` verifies that no resolved secret value appears in the tree (scan for Keychain values).
- Credentials: entirely the user's git setup (SSH agent, credential helper). `GIT_TERMINAL_PROMPT=0` turns auth failures into actionable errors with a "test connection" button.

## 14. Performance budgets (release build, M1 Mac, 1,000 skills + 50 other items, 20 projects)

| Metric | Budget |
|---|---|
| Launch → first paint with populated matrix | ≤ 300 ms (p95) |
| Background stamp catch-up (no changes) | ≤ 150 ms; CPU idle afterwards |
| Toggle (UI feedback) | ≤ 16 ms (optimistic) |
| Toggle → on disk (skill link) | ≤ 50 ms |
| External file change → row updated | ≤ 500 ms |
| Scroll matrix (5,000 rows × 12 cols) | 120 fps, no frame > 8 ms |
| Filter-as-you-type over 5,000 items | ≤ 16 ms per keystroke |
| Idle CPU / wakeups | ~0% / no timers except the scheduled update and sync checks |
| Memory (steady) | ≤ 150 MB |
| `skm list` (CLI cold) | ≤ 50 ms |
| Set edit → all using projects rewritten (20 projects, copy mode, 10 skills each) | ≤ 1 s, in the background |

CI runs a benchmark suite (T-100) on a synthetic HOME and fails on regressions > 20%.

## 15. Error handling

- `skm-core` uses one `Error` enum (thiserror) with stable `code()`s, such as:
  - `E_FOREIGN_TARGET`, `E_CONFIG_PARSE`, `E_CONFIG_CHANGED`;
  - `E_GIT_AUTH`, `E_GIT_MISSING`;
  - `E_NETWORK`, `E_LOCKED`;
  - `E_INVALID_SKILL`, `E_NAME_COLLISION`, `E_SECRET_MISSING`, `E_SECRET_IN_PROJECT`, `E_PROJECT_EDITED`.

  Each error carries a human message plus an optional `fix` hint.
- **Refusal is an error, never a silent success.** For example, "I didn't replace your file" is reported, not swallowed.
- Partial batch failure: every action is independent and recorded. The result lists done / skipped / failed, and Undo reverts the done part.
- Agent config parse failure: we never write to a file we can't parse. The cell shows "can't read <file>: <line>:<col>" with **Open file**.
- CLI: exit code 0 = ok; 1 = error; 2 = usage; 3 = completed with issues. `--json` prints `{"ok":false,"code":"E_…","message":"…","fix":"…"}`.
- Logs: `logs/skillsmgr.log` (tracing, daily rotation, 7 files). `skm doctor --bundle` zips logs plus a redacted config for bug reports.

## 16. CLI surface (`skm`)

Global flags: `--json`, `--dry-run`, `--yes`, `--home <dir>`, `-q`/`-v`.

```
skm status                                 # agents, scopes, counts, pending updates, sync state
skm list [--kind skill|mcp|hook|subagent|plugin] [--scope global|<path>] [--foreign]
skm show <item>                            # metadata, source, where active, problems
skm add <source> [--skill <name>]… [--kind …] [--on global|<path>] [--set <name>]
skm search <query>                         # skills.sh
skm remove <item>                          # to trash; refuses if active unless --off
skm on  <item> [--scope global|<path>] [--agent <id>|*]…   # skills: scope only; mcp/hook/subagent: scope + agents; plugin: global + agents
skm off <item> [--scope …] [--agent …]
skm adopt <path|--all> [--scope …]         # foreign → library
skm set list|show <set>|create <set>|delete <set>|add <set> <item>… [--agent <id>|*]…|rm <set> <item>…   # items may be skills, mcp, hooks, subagents
skm project list|add <path>|remove <path> [--clean]|use <path> <set>…|unuse <path> <set>…|mode <path> copy|link|pause <path>|resume <path>
skm mcp add <name> [--http <url>] [-- <cmd> <args>…] [--env K=V|--secret K]   # convenience over `add`
skm hook add <name> --event <E> [--matcher <m>] -- <command>
skm subagent add <name> --from <file.md>
skm agents [detect]                        # registry + detection + enabled
skm agent enable|disable <id>
skm update check|list|diff <item>|apply <item>… [--theirs|--ours]|ignore <item>
skm sync init [--remote <url>]|status|now|resolve
skm undo [<op-id>]
skm doctor [--reindex] [--bundle]
```

The CLI and the app call the same `skm-core` functions. A CLI command is a thin parse → core call → render.

## 17. UI (GPUI)

- **Main view: the matrix.**
  - Left sidebar: kind tabs (Skills · MCP · Hooks · Subagents · Plugins), Scopes (Global, then projects), Sets, Updates (n), Activity.
  - Top bar: scope switcher, filter field (`/`), "Add…" (⌘N).
  - Rows: items (virtualized DataTable, fixed first column, sortable).
  - Columns: **On** (the scope toggle) + one column per enabled agent.
  - Skills: agent cells show *visibility*, computed and not toggleable:
    - ● reads `.agents/skills`;
    - — not supported (Claude Code: "reads only ~/.claude/skills", with links to the issues);
    - ! problem.
  - MCP, hooks, subagents and plugins: agent cells are toggles, in the selected scope (Global or a project; plugins Global only). "—" means unsupported, with a tooltip reason (e.g. "OpenCode has no hooks"). "◌ via …" means visible through a shared file (e.g. Copilot via `.mcp.json`). A small "trust" marker links to the agent-specific trust hint.
- **Detail pane** (right, toggle ⌘I):
  - frontmatter, source and rev;
  - where it's active;
  - problems;
  - files tree with a read-only preview (gpui-component Markdown/code view);
  - actions: open in Finder, open in $EDITOR, Update, Remove.
- **Discover sheet:** skills.sh search, paste URL / owner/repo, local folder picker. It shows a preview before install.
- **Projects:** add via folder picker or drag-and-drop. Each project shows its sets, direct items, mode (copy default / link), auto-sync state (pause/resume, pending count), required env vars, and a "what will be written" plan preview (files + git status impact) before the first write.
- **Sets:** a membership editor across kinds (skills, MCP, hooks, subagents), with agent selection for non-skill kinds.
- **Updates:** a list with a diff viewer (side-by-side, `similar`-based).
- **Activity:** the op log with Undo.
- **Command palette** (⌘K) over all actions. The app is fully keyboard-drivable: j/k move, space toggles, enter opens detail.
- **Visual direction** (to avoid generic AI aesthetics):
  - dense, typographic and quiet, closer to Zed or Linear than to a dashboard;
  - system font (SF Pro) for text, SF Mono / JetBrains Mono for paths and names;
  - neutral greys, one accent color, state communicated by glyph + color (color-blind safe);
  - 1 px hairlines, 4 px radius max;
  - no gradients, glassmorphism, emoji, oversized cards or purple-blue hero banners;
  - light/dark follow the system.

  Theme tokens live in `theme.rs`; components come from `gpui-base` (unstyled) and are styled by us.
- **Never blocking:** every long action shows inline progress on its row. There are no modal spinners and no full-screen loading state after first launch. First launch shows an empty-state checklist: agents detected (with a note that Claude Code doesn't read `.agents/skills`) → adopt existing skills and MCP servers → done.

## 18. Testing

- **Unit:** parsers (frontmatter, source specs), renderers (golden files per agent dialect), event map, hash algorithm (fixtures cross-checked against npx `computedHash`).
- **Engine property tests** (`proptest`): random foreign files and links + random desired state → the plan never touches a foreign target, and apply then re-plan yields an empty plan (idempotence).
- **Integration:** a fake `HOME` and fake project repos per test (copy mode: assert files + `skills-lock.json`; `npx skills experimental_install` cross-check in a manual test) (tempdir) with registry path overrides; a git sync test against a local bare remote; CLI snapshot tests (`insta`).
- **UI:** gpui-component headless tests for matrix interactions, plus a manual macOS checklist per release.
- **Perf:** a criterion benchmark plus a `skm bench` synthetic generator (T-100).

## 19. Portability notes (Linux/Windows later)

Platform-specific parts are isolated:
- secrets: Keychain → `keyring` (libsecret / Credential Manager);
- symlinks: Windows needs Developer Mode or junctions for dirs. Junctions work for the global links; projects already default to copy mode;
- watcher backend limits: inotify `max_user_watches` is shown in doctor;
- packaging.

Core paths go through `paths.rs`; nothing outside it hard-codes `~`.
