# Roadmap

Status: draft v2 (after Sander's answers, 31 Oct 2026 19:08 Europe/Amsterdam) · Sizes and ticket IDs are in `tickets.md` (62 tickets). Durations assume one focused developer (Sander + agents). They are rough and only used for ordering.

Ordering principle: de-risk GPUI first. Then build the **core + CLI** end to end (usable on its own, including projects in committed-copy mode), then the GUI on top, then the other kinds at Global *and* Project scope, then updates and sync, then hardening. Each milestone ends in something Sander can use daily.

Fixed scope decisions:
- no Claude Code skills bridge (Claude won't see managed skills until it supports `.agents/skills`; anthropics/claude-code#31005, #16345);
- skills live in the `~/.skillsmgr` library and are linked into `~/.agents/skills`;
- project default = committed copies + `skills-lock.json`;
- MCP, hooks and subagents get project scope in v1;
- skills.sh is search-only.

---

## M0: Foundations and spike (≈1 week) · T-001…T-004

Goal: prove GPUI is viable before committing to it.
- Workspace skeleton, CI (macOS + Linux core tests), lint/format.
- **Spike:** gpui-kit window with a virtualized 5,000 × 12 DataTable (scroll fps, filter latency, styling freedom).
- Paths/config/`SKILLSMGR_HOME`, error type, logging.

**Exit criteria**
- [ ] The spike hits 120 fps on Sander's Mac and filters 5k rows in ≤ 16 ms per keystroke, with a custom (non-stock) theme. GO/NO-GO recorded in `decisions.md`. Fallback: plain `uniform_list` grid.
- [ ] `cargo test` green on macOS + Linux CI.

## M1: Core skills engine, projects + CLI (≈4 weeks) · T-010…T-032

Goal: Sander can manage global and project skills with the `skm` CLI alone, safely. Projects default to committed copies.
- SQLite store + migrations, library layout + `library.toml` (multi-kind sets), registry (6 agents, user + project targets for every kind), detection.
- Frontmatter index + stamps, watcher (including project files), startup catch-up.
- Engine: classification, ledger, plan/apply, trash, undo, lock.
- Global on/off (links into `~/.agents/skills`), foreign detection + adopt (with npx lock provenance).
- Sources: git, owner/repo, @skill and local; install flow; skills.sh search.
- Projects + multi-kind sets model; **copy mode (default) + npx-compatible `skills-lock.json`**; **auto-sync** with Edited safety and pause; optional link mode + `.git/info/exclude`; re-attach by content hash.
- CLI: status/list/show/add/search/remove/on/off/adopt/set/project/agents/undo/doctor.

**Exit criteria**
- [ ] On a fake HOME, the integration suite passes:
  - install → on globally → link in `~/.agents/skills`;
  - `~/.claude/skills` untouched;
  - a set applied to 2 projects in copy mode → copies + `skills-lock.json`;
  - editing the set rewrites both projects;
  - a teammate-edited copy is left alone and flagged Edited;
  - global skills are subtracted from projects.
- [ ] `npx skills experimental_install --frozen-lockfile` restores a copy-mode project's skills in a clean clone (manual).
- [ ] Property test: no plan ever touches a foreign target; apply then re-plan is empty.
- [ ] The app never stages or commits in a project repo (asserted in tests).
- [ ] `skm list` ≤ 50 ms cold with 1,000 synthetic skills.
- [ ] Sander migrates his real skills with `skm adopt --all --dry-run`, then for real, with zero loss (verified by hash).

## M2: GPUI app MVP for skills (≈3 weeks) · T-040…T-049

Goal: the daily-driver GUI for skills and projects.
- App shell, theme tokens, sidebar, scope switcher.
- Skills matrix (virtualized), visibility cells (Claude Code shown as "not supported" with issue links), on/off toggles, filter, keyboard navigation, palette.
- Detail pane; Discover sheet (search, URL, local folder) with preview.
- Projects view (mode, auto-sync pause and pending changes, Edited resolution, plan preview with git status impact) and Sets view.
- Activity + Undo; core → GUI event channel (row-level updates); first-run onboarding.

**Exit criteria**
- [ ] First paint ≤ 300 ms with 1,000 skills; external change → row update ≤ 500 ms (bench + manual).
- [ ] Every M1 CLI action is possible in the GUI; CLI changes appear in the open app within 500 ms.
- [ ] Onboarding on a fresh macOS user account completes in < 1 min and writes nothing outside `~/.skillsmgr` and `~/.agents/skills`.
- [ ] Design review against the visual direction (spec §17) signed off by Sander.

## M3: MCP, hooks, subagents (Global + Project) and plugins (≈4–5 weeks) · T-060…T-071

Goal: one matrix for every resource type, at Global and Project scope (plugins Global only), with sets spanning all four project-capable kinds.
- Config edit layer for user and project files (JSON preserve-order, JSONC, toml_edit, create-if-absent, hash-checked writes, Claude CLI path, link-mode excludes).
- Secrets: Keychain at Global scope; **env references only at Project scope**.
- Shared-file planner (`.mcp.json` Claude+Copilot, `.claude/agents` Claude+Cursor, `.codex/agents` Codex+Cursor) and per-agent trust hints.
- MCP (6 dialects incl. OpenCode v1/v2), subagents (5 renderers + Cursor rule) and hooks (event map, 5 renderers): each at both scopes, plus adopt existing.
- Sets + project auto-sync for MCP, hooks and subagents.
- Plugins: inventory, enable/disable via CLIs, local marketplace install (Global).
- Matrix kind tabs for both scopes; CLI parity.

**Exit criteria**
- [ ] Golden-file tests per dialect × scope; round-trip tests prove unknown keys and comments are preserved.
- [ ] Enabling an MCP server in a project for Claude/Codex/Cursor/Copilot writes `.mcp.json`, `.codex/config.toml`, `.cursor/mcp.json` and `.github/mcp.json` correctly. The server is listed by `claude mcp list` and `codex mcp list` (trusted project) and appears in Cursor (manual check).
- [ ] No literal secret appears in any project file or under `library/` (automated scan).
- [ ] Adding an MCP server, a hook and a subagent to a set updates every project using it; foreign and Edited entries are untouched; undo works.
- [ ] Unsupported cells are visible as "—" with a reason (e.g. OpenCode hooks); shared-file visibility shows "via …".

## M4: Updates (≈1.5 weeks) · T-080…T-082

Goal: safe upstream updates that flow into projects.
- Grouped update check, scheduler, notifications.
- Diff view (upstream / local / merged), 3-way apply, conflict resolution, ignore version; propagation to copy-mode projects (copies + lock).

**Exit criteria**
- [ ] The test matrix (unchanged / local-only / upstream-only / both / added locally / deleted upstream) behaves per spec §12.4, with no lost bytes.
- [ ] One fetch per repo regardless of how many skills come from it.
- [ ] After an approved update, copy-mode projects have new copies and updated `computedHash` values in `skills-lock.json`.

## M5: Git sync (≈2 weeks) · T-090…T-094

Goal: identical setup on two Macs.
- `sync init/connect`, auto commit/fetch/merge/push loop, conflict resolver UI + CLI, secret guard, post-pull reconcile (global + projects, respecting pause), "missing secrets" prompt.

**Exit criteria**
- [ ] Two fake HOMEs against a local bare remote converge after concurrent edits. Conflicting edits surface in the resolver; nothing is lost (backup in trash).
- [ ] Real test: Sander's laptop and desktop reach the same global state in ≤ 2 min. Registering an already-cloned copy-mode project on the second machine shows no Foreign rows (re-attach).

## M6: Hardening and release v1 (≈1.5 weeks) · T-100…T-104

- Bench suite in CI with a regression gate; perf pass against every budget (including set-edit propagation).
- Signing, notarization, DMG, Homebrew cask (`skillsmgr`) + formula (`skm`), auto-update decision.
- Docs: README, concepts page (including copy vs link mode, and why Claude Code doesn't see skills yet), CLI reference (generated from clap).

**Exit criteria**
- [ ] All spec §14 budgets met on an M1 Mac.
- [ ] 2 weeks of daily use by Sander (3+ real repos in copy mode with multi-kind sets) with zero data-loss incidents; open P0/P1 bugs = 0.
- [ ] Notarized build installs and launches on a clean macOS account.

## Later

- v1.1: plugin leftovers, if any.
- v2:
  - Linux/Windows;
  - MCP server for the core;
  - more registry agents;
  - `.agents/agents` target if it becomes standard;
  - project-scope plugins;
  - Claude Code skills as soon as it reads `.agents/skills` (a registry data change).

## Critical path

M0 spike → M1 engine (classification/ledger/plan/apply) → M1 projects/copy mode/auto-sync → M2 matrix → M3 config edit layer → shared-file planner → MCP/hooks/subagents → sets for all kinds → M5 sync. Updates (M4) can run in parallel with M3 once M1 is done.
