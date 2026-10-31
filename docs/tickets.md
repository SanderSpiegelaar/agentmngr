# Tickets

Status: draft v2 (after Sander's answers) · 31 Oct 2026 (Europe/Amsterdam) · The same data is in `tickets.csv`. Tickets are listed in a valid build order: every dependency appears earlier.

Sizes:

- **S** ≤ 1 day
- **M** 2–3 days
- **L** 4–5 days

Total: 62 tickets (22 S, 35 M, 5 L).

## M0: Foundations and spikes

### T-001 · Cargo workspace + CI skeleton  `[S]`

**Depends on:** —

Create workspace with skm-core, skm-cli, skm-app crates (edition 2024, pinned toolchain). CI: fmt, clippy -D warnings, test on macOS + Linux (core/cli only on Linux).

**Acceptance:** cargo build/test pass on macOS and Linux; clippy clean; `skm --version` and an empty skillsmgr window binary build.

### T-002 · Paths, config.toml, SKILLSMGR_HOME  `[S]`

**Depends on:** T-001

paths.rs: resolve home via --home > SKILLSMGR_HOME > ~/.skillsmgr; create layout (library/, cache/, trash/, logs/). config.toml load/save with defaults and agent path overrides. Nothing else may hard-code ~.

**Acceptance:** Unit tests for resolution order; first run creates layout idempotently; `skm doctor` prints resolved paths.

### T-003 · Spike: gpui-kit virtualized matrix  `[M]`

**Depends on:** T-001

Throwaway branch: gpui-kit (exact pin) window with DataTable of 5,000 rows x 12 columns, fixed first column, glyph cells, filter field, custom theme tokens (no stock look). Measure scroll fps and filter latency. Record GO/NO-GO and fallback (uniform_list grid) in decisions.md.

**Acceptance:** 120 fps scroll on Sander's Mac; filter <=16 ms/keystroke; screenshot + numbers in decisions.md; pinned gpui-kit version chosen.

### T-004 · Error type, logging, events skeleton  `[S]`

**Depends on:** T-001

error.rs (thiserror enum with stable codes + fix hints, incl. E_SECRET_IN_PROJECT, E_PROJECT_EDITED), tracing to logs/ with daily rotation, CoreEvent enum and channel plumbing.

**Acceptance:** Error codes documented; log file created under $SKILLSMGR_HOME/logs; unit test for code() stability.

## M1: Core skills engine + CLI

### T-010 · SQLite store + migrations  `[M]`

**Depends on:** T-002, T-004

rusqlite (bundled) store thread as single writer with command channel; read connection pool; WAL; user_version migrations; schema per spec §9 (scopes.mode default 'copy', auto_sync, multi-kind project_items, ledger 'lock'/'exclude').

**Acceptance:** Migrations apply from 0 and are idempotent; concurrent reads during writes work; DB deleted -> recreated on next start.

### T-011 · library.toml model + read/write  `[M]`

**Depends on:** T-002

Serde model for library.toml (schema=1, global, sets with skills/mcp/hooks/subagents and agent lists or '*', skills provenance); deterministic sorted output; toml_edit-based writes that preserve comments.

**Acceptance:** Round-trip test preserves comments and ordering; two independent edits to different items produce a clean git merge in a test; multi-kind set parses and validates agent ids.

### T-012 · Agent registry (data) + detection  `[M]`

**Depends on:** T-002

Embedded registry/agents.toml for Claude Code, Codex, Cursor, Gemini CLI, OpenCode, Copilot CLI with user AND project targets for skills/mcp/hooks/subagents (paths per spec §11.5), plugins driver, also_read_by, trust hints, CLAUDE_CONFIG_DIR/COPILOT_HOME, config.toml overrides; Claude skills marked reads_universal=false with issue links. Detection via paths and bin: lookups (no process spawn on the hot path).

**Acceptance:** Registry validates at load (every project path relative, every also_read_by id exists); detection test on fake HOME; `skm agents` lists detected/enabled; adding a 7th agent requires only TOML.

### T-013 · Frontmatter parser + skill validation  `[S]`

**Depends on:** T-004

Read SKILL.md frontmatter only (stop at closing ---, 64 KB cap) with serde_yaml_ng; validation per Agent Skills spec (name rules, dir match, description length) as warnings.

**Acceptance:** Fixtures for valid/invalid/missing/huge files; 10k parses < 200 ms.

### T-014 · Content hash (npx computedHash compatible)  `[S]`

**Depends on:** T-001

SHA-256 over files sorted by relative path, feeding relPath then bytes; skip .git; parallel file reads.

**Acceptance:** Hash equals npx skills computedHash on 3 fixture skills (cross-checked with vercel-labs/skills code).

### T-015 · Indexer + stamps  `[M]`

**Depends on:** T-010, T-011, T-013

Build items table from library/ (all kinds); stamp = mtime+size of item dir/file and SKILL.md; incremental reindex of given item ids; full reindex command.

**Acceptance:** Startup loads index without touching library files; catch-up of 1,000 unchanged items <=150 ms; `skm doctor --reindex` rebuilds identical rows.

### T-016 · Target classification  `[S]`

**Depends on:** T-002

classify(path) -> Absent | OurLink(item) | ForeignLink | RealDir | RealFile, resolving symlinks relative to $SKILLSMGR_HOME/library; content-hash comparison helper for copies.

**Acceptance:** Unit tests for each class incl. broken links, relative links, links into another tool's dir, copy whose hash matches a library item.

### T-017 · Engine: plan/apply, ledger, trash, lock  `[L]`

**Depends on:** T-010, T-016

Action/Plan types; scoped planning; apply with inter-process lock (held only during apply), re-classification before each action, atomic temp+rename writes, ledger rows, trash moves, ops log with inverse actions.

**Acceptance:** Property test (proptest): random foreign trees + desired state -> no action targets foreign paths; apply then re-plan = empty; kill mid-apply leaves no half-written targets.

### T-018 · Undo  `[S]`

**Depends on:** T-017

Replay inverse actions for an op (last 50); refuse if targets changed since.

**Acceptance:** Undo of on/off/adopt/remove/project apply restores exact prior state (hash-verified); conflicting undo reports E_CONFIG_CHANGED.

### T-019 · Global skills on/off  `[S]`

**Depends on:** T-015, T-017

Desired global skills from library.toml -> symlinks in ~/.agents/skills pointing into library/skills; off removes our link only; content never moves. Nothing is ever written to ~/.claude/skills.

**Acceptance:** Toggle -> link exists/removed in <=50 ms; foreign entry with same name -> PlanIssue, not overwritten; test asserts ~/.claude/skills is untouched.

### T-020 · Foreign detection + adopt (+npx lock provenance)  `[M]`

**Depends on:** T-019, T-011

List foreign entries in ~/.agents/skills and project .agents/skills; read ~/.agents/.skill-lock.json (v3) and skills-lock.json (v1) for provenance; adopt = copy->verify hash->replace with link (global) or keep copy (project copy mode)->original to trash.

**Acceptance:** Adopt preserves bytes (hash), records source from lock when present; nothing changes without explicit adopt; `--dry-run` lists exact actions.

### T-021 · Watcher  `[M]`

**Depends on:** T-015, T-019

notify + notify-debouncer-full (250 ms) on library/, ~/.agents/skills, registered projects' .agents/skills + skills-lock.json + registry project config paths (parent-dir filter), user agent config files, .changed; ignore list; path->item/target prefix map; emit ItemsChanged for affected ids only; FSEvents rescan flag handling.

**Acceptance:** Editing one SKILL.md reindexes exactly one item; editing a project .mcp.json re-classifies only that project's entries; git checkout of 400 files -> one batch; no polling timers (verified by idle wakeup count).

### T-022 · Source spec parser  `[S]`

**Depends on:** T-004

Parse owner/repo, owner/repo@skill, GitHub tree URLs, generic git URLs (#ref, --path), local paths, skills.sh URLs into Source.

**Acceptance:** Table-driven tests for 20+ inputs incl. ssh and GitLab URLs; clear error for ambiguous input.

### T-023 · Git runner (system git)  `[S]`

**Depends on:** T-004

Thin wrapper over std::process::Command for git with timeouts, GIT_TERMINAL_PROMPT=0, stderr capture -> typed errors (E_GIT_AUTH, E_GIT_MISSING, E_NETWORK); version check >=2.30. Also used read-only for project repos (git ls-files to know if a file is tracked); never commits in project repos.

**Acceptance:** Tests against local bare repos; missing git yields E_GIT_MISSING with Xcode CLT hint; API has no commit/add function usable on project scopes (enforced by type).

### T-024 · Install flow (git + local)  `[M]`

**Depends on:** T-022, T-023, T-014, T-011, T-017

Blobless shallow bare mirror in cache/git; ls-tree discovery of SKILL.md; preview model; git archive extract to temp -> validate -> rename into library; provenance+base_hash into library.toml; base snapshot; collision rename/replace; local folder import.

**Acceptance:** Install from a fixture repo with 3 skills installs only selected; second install of same repo reuses mirror; collision prompts; local import copies bytes exactly.

### T-025 · skills.sh search client  `[S]`

**Depends on:** T-004

ureq GET /api/search?q=&limit= with SKILLSMGR_SKILLS_API / SKILLS_API_URL override, 10-min cache, map result to owner/repo@skill. Search only; no leaderboard.

**Acceptance:** Mocked HTTP tests; offline -> E_NETWORK without blocking; live smoke test behind feature flag.

### T-026 · Projects + multi-kind sets model + effective-set calc  `[M]`

**Depends on:** T-019, T-011, T-010, T-012

Register projects (mode copy by default, auto_sync on); direct project items of any kind; sets used by projects; effective(kind) = union(sets) + direct - global-for-same-agent; '*' agent expansion against enabled agents; set edits trigger scoped replans for all using projects. Skills targets only in M1 (non-skill targets plug in during M3 via the same planner).

**Acceptance:** Effective-set unit tests incl. global subtraction and '*' expansion; nothing created in a repo until first activation; a set edit produces plans for exactly the projects using it.

### T-027 · Project copy mode (default) + skills-lock.json  `[M]`

**Depends on:** T-026, T-014, T-017

Copy mode: real copies in <repo>/.agents/skills/<name> tracked by hash; regenerate npx-compatible skills-lock.json v1 (sorted keys, computedHash, relative local sources), preserving lock entries we don't own; never stage or commit.

**Acceptance:** `npx skills experimental_install --frozen-lockfile` in a clean clone restores the same skills (manual); foreign lock entries survive regeneration byte-for-byte; git status shows only expected working-tree changes.

### T-028 · Project auto-sync + Edited safety + pause  `[M]`

**Depends on:** T-027, T-021, T-018

Library change or set edit -> background plan+apply for every affected project: rewrite copies, regenerate lock (copy mode) or relink (link mode). Hash mismatch vs ledger -> Edited (E_PROJECT_EDITED), never overwritten; actions pull-into-library / overwrite / keep. Per-project pause with pending count. Each auto-apply logged in ops with Undo.

**Acceptance:** Updating a library skill rewrites copies + lock in 3 fixture projects within the §14 budget; a teammate-edited copy is left untouched and reported; paused project accumulates pending changes and applies on resume; undo restores previous copies + lock.

### T-029 · Project link mode (optional) + .git/info/exclude  `[S]`

**Depends on:** T-026, T-017, T-023

Link mode: symlinks to library + managed block in .git/info/exclude for paths we create (skills now; config files in M3); switch copy<->link as a previewed plan (lock written/removed).

**Acceptance:** git status clean in link mode; switching modes round-trips without data loss; exclude block only contains our entries and is removed when empty.

### T-030 · Re-attach existing project content by hash  `[S]`

**Depends on:** T-027, T-020

On project registration (new machine or teammate checkout), entries in .agents/skills whose content hash matches a library item are recorded as ours in the ledger; skills-lock.json used to suggest sets; mismatches stay Foreign/Edited.

**Acceptance:** Cloning a copy-mode fixture project onto a second fake HOME with the same library yields zero Foreign rows; a modified copy shows Edited.

### T-031 · CLI v1 commands  `[L]`

**Depends on:** T-019, T-020, T-024, T-025, T-026, T-027, T-028, T-029, T-030, T-018

clap CLI per spec §16 for status/list/show/add/search/remove/on/off/adopt/set/project (incl. mode copy|link, pause/resume)/agents/agent/undo/doctor with --json, --dry-run, --yes, exit codes 0/1/2/3; touches .changed after apply.

**Acceptance:** insta snapshot tests for human and JSON output; every mutating command supports --dry-run with identical plan; `skm list` <=50 ms cold at 1,000 skills.

### T-032 · skm doctor  `[S]`

**Depends on:** T-031, T-023, T-012

Checks: git present/version, data dir writable, registry overrides valid, broken links, unparsable agent configs (user + project), foreign conflicts, Edited project entries, inotify limits (Linux), secrets scan (hooked in M3/M5); --bundle zips logs + redacted config.

**Acceptance:** Each check has a fixture that triggers it and a fix hint; --bundle contains no secret values.

## M2: GPUI app MVP (skills)

### T-040 · App shell, theme tokens, core→UI event channel  `[M]`

**Depends on:** T-003, T-021

skm-app main window, sidebar, top bar, theme.rs (light/dark, one accent, mono for paths), CoreEvent -> GPUI entity updates on the foreground executor; core calls on background executor.

**Acceptance:** Window opens with populated sidebar from DB; ItemsChanged updates only affected rows (instrumented); no blocking calls on main thread (debug assertion).

### T-041 · Skills matrix view  `[L]`

**Depends on:** T-040, T-019, T-026

Virtualized DataTable: name column fixed, On toggle, agent columns with visibility glyphs (reads .agents/skills / not supported with reason / problem), sorting, filter, scope switcher (Global/project), optimistic toggles reconciled on ApplyDone. Claude Code column shows 'not supported (reads only ~/.claude/skills)' with links to anthropics/claude-code#31005 and #16345.

**Acceptance:** Meets spike perf numbers with real data; toggle feedback <=16 ms; external change -> row update <=500 ms; glyph+color states legible in light/dark; Claude tooltip shows issue links.

### T-042 · Keyboard navigation + command palette  `[M]`

**Depends on:** T-041

j/k/space/enter//, ⌘K palette listing all actions, ⌘N add, ⌘I detail; focus management.

**Acceptance:** Every M1 CLI action reachable via palette; full flow (find skill, toggle, open detail) without mouse.

### T-043 · Detail pane  `[M]`

**Depends on:** T-041

Frontmatter, source+rev, where active (global + projects), problems, file tree, read-only Markdown/code preview, actions (Finder, $EDITOR, Remove, Update).

**Acceptance:** Opening detail for a 13k-file skill stays responsive (lazy tree); preview renders SKILL.md correctly.

### T-044 · Discover sheet (search, URL, local)  `[M]`

**Depends on:** T-041, T-024, T-025

skills.sh search results, paste owner/repo or URL, local folder picker, preview list with checkboxes, size/scripts warnings, install + optional activation target (Global, project, set).

**Acceptance:** Install from each source type works from the GUI; long clones show inline progress and can be cancelled.

### T-045 · Projects and Sets views  `[M]`

**Depends on:** T-041, T-026, T-028, T-029

Add project (picker/drag-drop); per-project sets, direct items, mode (copy default / link), auto-sync pause/resume + pending count, Edited entries with pull/overwrite/keep; plan preview incl. git status impact before first write. Sets CRUD with a membership editor across kinds (skills now; MCP/hooks/subagents enabled in M3).

**Acceptance:** First activation in a repo shows the exact files to be created; editing a set visibly updates all projects within 500 ms (UI) and lists rewritten files in Activity.

### T-046 · Activity view + Undo  `[S]`

**Depends on:** T-041, T-018

List ops (incl. auto-sync applies) with summaries, done/skipped/failed details, Undo button.

**Acceptance:** Undo from GUI matches `skm undo` behaviour; CLI ops appear live.

### T-047 · Foreign/conflict UX  `[S]`

**Depends on:** T-041, T-020

Foreign rows with Adopt; Conflict badges with explanation and explicit 'Replace (original to Trash)'.

**Acceptance:** No path to overwrite a foreign target without the explicit replace action; adopt from GUI is hash-verified.

### T-048 · First-run onboarding  `[M]`

**Depends on:** T-041, T-020

Empty-state checklist: detected agents (with a plain note that Claude Code doesn't read .agents/skills yet) -> adopt existing skills (preview) -> done.

**Acceptance:** Fresh macOS account completes onboarding in <1 min; skipping any step leaves the system untouched; no writes outside ~/.skillsmgr and ~/.agents/skills.

### T-049 · Design review pass  `[S]`

**Depends on:** T-041, T-043, T-044, T-045

Review against spec §17 visual direction; fix spacing/typography/states; screenshot set for docs.

**Acceptance:** Sander signs off on screenshots; no stock component styling left on main surfaces.

## M3: MCP, hooks, subagents, plugins

### T-060 · Config edit layer (user + project files)  `[L]`

**Depends on:** T-017, T-012, T-023

Surgical key-path upsert/remove for JSON (preserve order), JSONC (OpenCode), TOML (toml_edit); create-if-absent for project files (dirs too); read->edit->hash-check unchanged->atomic write->previous copy to trash; ledger 'entry' rows; parse failure = no write. In link mode, files we create are added to the exclude block; tracked files are edited in place and flagged in the plan.

**Acceptance:** Round-trip fixtures prove unknown keys, order and comments preserved; concurrent external modification -> E_CONFIG_CHANGED; invalid file never written; link-mode project shows created config files excluded.

### T-061 · Keychain secrets + project env references  `[S]`

**Depends on:** T-004

secrets trait + macOS security-framework impl; ${secret:NAME} resolution at Global scope; at Project scope render as env references per dialect, refuse (E_SECRET_IN_PROJECT) where no syntax exists; missing-secret prompts; required-env list per project.

**Acceptance:** Secret round-trip test (macOS CI); no literal secret ever appears in a project file or library file (scan test); Copilot project secret refused with hint.

### T-062 · Shared-file planner + trust hints  `[M]`

**Depends on:** T-060, T-012, T-026

Planner support for also_read_by (.mcp.json: Claude+Copilot; .claude/agents: Claude+Cursor; .codex/agents: Codex+Cursor): no double writes, derived 'via …' visibility, Copilot project MCP to .github/mcp.json. Per-agent trust hints per project (Codex trusted projects + hook trust, Copilot folder trust, Claude .mcp.json approval, Gemini trusted folders).

**Acceptance:** Planner tests: enabling an MCP server for Claude+Copilot in a project writes .mcp.json and .github/mcp.json once each with correct dialects; Cursor subagents covered via .claude/agents without a .cursor/agents copy; trust hint appears once per project/agent.

### T-063 · MCP canonical model + 6 renderers (Global + Project)  `[L]`

**Depends on:** T-060, T-061, T-062

library/mcp/<name>.json model; dialect renderers for claude, codex, cursor, gemini, opencode (v1 mcp.<n> and v2 mcp.servers.<n>, detected), copilot incl. dropped-field reporting and name rules; user + project targets per spec §11.5; Claude user scope via `claude mcp add-json/remove` when CLI present.

**Acceptance:** Golden files per dialect x scope; `claude mcp list` and `codex mcp list` show the server after enable at both scopes (manual, Codex project trusted); disabling removes only our entry.

### T-064 · MCP adopt existing servers (user + project)  `[M]`

**Depends on:** T-063

Scan each agent's user config and registered projects' config files for servers -> foreign entries; adopt creates canonical file, offers to move literal secrets to Keychain (Global) or env refs (Project), links ledger to existing entries.

**Acceptance:** Adopting a server configured in 3 agents yields one canonical item enabled for those 3 at the same scope with no config diff besides secrets handling.

### T-065 · Subagents: canonical + renderers (Global + Project)  `[M]`

**Depends on:** T-060, T-062

library/subagents/<name>.md; render to Claude MD, Codex TOML, Gemini MD (allowed keys), OpenCode MD, Copilot .agent.md at user and project paths (§11.5); Cursor-via-Claude/Codex rule; Edited detection with pull-into-library/overwrite.

**Acceptance:** Golden files per agent x scope; Cursor column shows 'via Claude' when Claude enabled; edited rendered copy is never overwritten silently.

### T-066 · Hooks: canonical + event map + renderers (Global + Project)  `[M]`

**Depends on:** T-060, T-062

library/hooks/<name>.json; full event map table in registry (research §5.2); renderers for Claude/Gemini settings.json (user + .claude/settings.json / .gemini/settings.json), Codex hooks.json (user + .codex/hooks.json; features.hooks check/offer), Cursor hooks.json v1 (user + project), Copilot own file (~/.copilot/hooks/skillsmgr.json, .github/hooks/skillsmgr.json); ${PROJECT_ROOT} rendering; exact-object ownership.

**Acceptance:** Golden files per agent x scope; unmapped events render nothing and report reason (OpenCode: no hooks); command shown and confirmed before first enable.

### T-067 · Sets + project auto-sync for MCP, hooks, subagents  `[M]`

**Depends on:** T-063, T-065, T-066, T-028

Extend effective-set planning and auto-sync to non-skill kinds: set entries with agent lists or '*', upsert/remove our entries in project config files and rendered subagent files on set or library change; Edited detection for entries; global duplicates subtracted per agent.

**Acceptance:** Adding an MCP server + hook + subagent to the 'rust' set updates 3 fixture projects' config files (correct per-agent files), leaves foreign and Edited entries untouched, and is undoable.

### T-068 · Plugins: inventory + enable/disable (Global)  `[M]`

**Depends on:** T-012, T-023

Read installed/enabled plugins for Claude (installed_plugins.json, enabledPlugins), Codex (config + cache), Cursor (read-only); enable/disable via claude/codex CLIs, toml_edit fallback for Codex. Not in sets; Global only.

**Acceptance:** Inventory matches `claude plugin list` on Sander's machine; toggling reflects in agent after restart (manual).

### T-069 · Plugins: local marketplace install  `[M]`

**Depends on:** T-068, T-024

Install plugin from git into library/plugins/<name>; regenerate .claude-plugin/marketplace.json and .agents/plugins/marketplace.json; register marketplace once via agent CLIs.

**Acceptance:** Plugin installed from a fixture repo is installable/enabled in Claude Code and Codex via the skillsmgr marketplace (manual).

### T-070 · Matrix tabs for MCP/hooks/subagents/plugins (both scopes)  `[M]`

**Depends on:** T-041, T-045, T-063, T-065, T-066, T-068, T-067

Kind tabs reuse the matrix with toggleable agent cells in the selected scope (plugins Global only), unsupported '—' with tooltip reason, 'via …' cells, trust markers, required env vars per project, detail pane per kind (rendered preview per agent); Sets editor gains non-skill kinds.

**Acceptance:** Enable/disable per agent from the matrix for each kind at Global and Project scope; tooltip reasons for every unsupported cell; set membership editable for all four kinds.

### T-071 · CLI for other kinds  `[S]`

**Depends on:** T-031, T-063, T-065, T-066, T-068, T-067

`skm mcp add`, `skm hook add`, `skm subagent add`, `--kind` support across list/show/on/off/adopt/remove, `--scope <path>` for mcp/hook/subagent, `skm set add <set> <item> --agent`.

**Acceptance:** Snapshot tests; parity with GUI actions at both scopes.

## M4: Updates

### T-080 · Update checker  `[M]`

**Depends on:** T-024, T-010

Group items by source repo; ls-remote, blobless fetch, compare subpath tree id; updates table; scheduler (5 min after launch, every 6 h, manual); ignore version.

**Acceptance:** One fetch per repo regardless of skill count; offline -> error state, no UI block; only subpath changes flagged.

### T-081 · 3-way update apply (+ project propagation)  `[M]`

**Depends on:** T-080, T-017, T-028

Per-file rules from spec §12.4 using base snapshot + git merge-file; conflicts returned as structured result; previous version to trash; provenance update; after apply, auto-sync propagates to copy-mode projects (copies + skills-lock.json).

**Acceptance:** Test matrix (unchanged/local-only/upstream-only/both/added-locally/deleted-upstream) passes with byte-exact expectations; copy-mode projects get new copies and updated computedHash in the lock.

### T-082 · Updates UI + notifications  `[M]`

**Depends on:** T-081, T-041

Updates sidebar count, row badges, diff viewer (upstream/local/merged via similar), per-file conflict choice, list of projects that will be rewritten, macOS notification (opt-in); CLI `skm update check|list|diff|apply|ignore`.

**Acceptance:** Apply after approval only; local edits visibly preserved in merged panel; CLI and GUI produce identical results.

## M5: Git sync

### T-090 · Sync init/connect  `[M]`

**Depends on:** T-023, T-011

`skm sync init [--remote]`: git init library/, .gitignore, empty-remote push, empty-local clone, both-non-empty merge with prior full backup to trash.

**Acceptance:** All three cases tested against local bare remotes; backup restorable.

### T-091 · Auto-sync loop  `[M]`

**Depends on:** T-090, T-017

Debounced commit (30 s), periodic fetch (10 min + startup), merge --no-edit, push; post-merge reconcile via plan/apply (global + projects, via project auto-sync rules); status events.

**Acceptance:** Two fake HOMEs converge after independent edits; foreign-blocked targets become issues not forced actions; paused projects are not touched.

### T-092 · Conflict resolver (CLI + GUI)  `[M]`

**Depends on:** T-091, T-041

Pause auto-sync on conflict; banner; per-file mine/theirs/open $EDITOR; `skm sync resolve`.

**Acceptance:** Conflicting edits on two machines resolved without data loss; auto-sync resumes after resolution.

### T-093 · Secret guard + missing-secret prompts  `[S]`

**Depends on:** T-091, T-061

Pre-commit scan of library for resolved Keychain values and common token patterns; block commit with explanation; after pull, prompt for placeholders missing in Keychain.

**Acceptance:** Planted secret blocks commit; new device prompts for each missing secret and renders configs after entry.

### T-094 · Large-file limits  `[S]`

**Depends on:** T-091

Warn >5 MB, block >50 MB files in library commits with clear message.

**Acceptance:** Fixture with 60 MB file blocks sync with actionable error.

## M6: Hardening and release

### T-100 · Bench suite + CI regression gate  `[M]`

**Depends on:** T-041, T-031

Synthetic HOME generator (1k/5k skills, 20 copy-mode projects); criterion benches for startup, catch-up, plan, filter, set-edit propagation; CI fails on >20% regression.

**Acceptance:** Bench report for all spec §14 budgets; gate demonstrated by an intentional slowdown PR.

### T-101 · Perf pass against budgets  `[M]`

**Depends on:** T-100

Profile and fix anything over budget (startup, scroll, watcher latency, auto-sync propagation, memory, idle wakeups).

**Acceptance:** All §14 budgets met on an M1 Mac; results recorded.

### T-102 · Packaging: sign, notarize, DMG, Homebrew  `[M]`

**Depends on:** T-049

cargo-packager/cargo-bundle app bundle, Developer ID signing, notarization, DMG; Homebrew cask (skillsmgr) + formula (skm); update strategy decision.

**Acceptance:** Notarized DMG installs and launches on a clean account; `brew install` works from a tap.

### T-103 · Docs  `[S]`

**Depends on:** T-031, T-049

README, concepts page (Library/Set/Scope/Agent, copy vs link mode, why Claude Code doesn't see skills yet), CLI reference generated from clap, troubleshooting from doctor codes.

**Acceptance:** Docs reviewed by Sander; every error code documented.

### T-104 · Dogfood + bug bash  `[M]`

**Depends on:** T-101, T-102, T-092, T-082, T-070

Two weeks of daily use on Sander's machines (incl. 3+ real repos in copy mode with multi-kind sets); triage and fix P0/P1.

**Acceptance:** Zero data-loss incidents; open P0/P1 = 0; release v1 tag prepared (no publishing without Sander).
