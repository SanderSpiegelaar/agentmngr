# PRD: skillsmgr

Status: draft v3 (all open questions answered, 31 Oct 2026 19:15) · 31 Oct 2026 (Europe/Amsterdam) · Technical detail lives in `spec.md`; evidence lives in `research.md`.

## 1. Problem

Developers now use several coding agents (Claude Code, Codex, Cursor, Gemini CLI, OpenCode, Copilot). Each agent loads **skills**, **MCP servers**, **hooks**, **subagents** and **plugins** from its own places, in its own formats. Keeping them consistent across agents, projects and machines is manual, error-prone work.

The existing open-source Skills Manager (Tauri + React, 7.1k stars) tackles this, but users hit four problems:
1. **Wrong defaults.** It ignores the industry-standard `~/.agents/skills` as a deploy target, writes Codex skills to the wrong folder, creates `.claude/skills-disabled` folders inside repos, and makes Cursor load skills twice.
2. **Slow.**
   - Startup blocks on a full re-index (90 s reported for one large skill).
   - Every file change triggers a full refresh plus 3-second polling.
   - Lists are not virtualized.
3. **Bad UI.** The concepts are confusing (preset vs enabled vs deployed), a webview sometimes shows a black screen, and the UI janks.
4. **Unsafe.** It lost local edits on update and deleted folders it didn't own.

## 2. Goals

- G1. One place to see and control, for every installed agent, which skills, MCP servers, hooks, subagents and plugins are active globally and per project.
- G2. Correct by default: skills are delivered once, to `.agents/skills` (the cross-agent standard), from a library in `~/.skillsmgr`. There are no per-agent skill folders.
- G3. Instant: a native app that paints in ≤ 300 ms, never scans before paint, and reacts to file changes within 0.5 s.
- G4. Safe: it never loses user data. Every change is previewable, undoable and limited to files the app owns.
- G5. Portable setup: named sets for project types (covering skills, MCP servers, hooks and subagents) that projects commit by default, plus git-based backup and sync of the library across machines.
- G6. A CLI that can do everything the app does (automation, dotfiles, CI).

## 3. Non-goals (v1)

- **Making Claude Code see managed skills.** Claude Code currently reads skills only from `~/.claude/skills` and `.claude/skills`, not `.agents/skills`. Until Anthropic ships support (open requests: [anthropics/claude-code#31005](https://github.com/anthropics/claude-code/issues/31005), [anthropics/claude-code#16345](https://github.com/anthropics/claude-code/issues/16345)), Claude Code won't see skills managed by skillsmgr. We add no bridge, links or other workaround. The matrix shows this as "not supported". Claude Code's MCP servers, hooks, subagents and plugins *are* managed.
- Linux and Windows builds. The core stays portable, but only macOS ships in v1.
- Project-scope plugins, and plugins in sets (plugins are Global-only in v1).
- An MCP server interface for the app itself (post-v1).
- Importing data from the old Skills Manager.
- Translating plugins between formats; Gemini extensions; OpenCode JS plugins.
- A skills.sh leaderboard, trending or curated view. v1 does search only, and there is no outreach to Vercel for API access.
- Authoring or editing skills inside the app beyond "open in editor".
- Committing to project repos on the user's behalf (the app only changes the working tree).
- Team or shared-cloud features, accounts or telemetry.
- Per-agent skill opt-out (a skill is on for a scope, for every agent that reads that scope).

## 4. Personas

- **P1, the multi-agent developer (Sander).** Uses Claude Code + Codex + Cursor daily on Rust and TypeScript repos and owns 50–300 skills. Wants different skill sets per project type and an identical setup on a laptop and a desktop. Lives in the terminal as much as in GUIs.
- **P2, the curator.** Collects skills from GitHub and skills.sh and tweaks them locally. Needs updates that don't clobber those tweaks.
- **P3, the team lead.** Wants a repo's skills, MCP servers, hooks and subagents committed so teammates get them, even teammates who don't use the app (skills via `skills-lock.json` / `npx skills`; the rest via the agents' own project files).

## 5. User stories

Format: As a [persona] I want… so that…. The IDs map to the requirements in §6.

Library and sources
- US-1 (P1) I want to add skills from `owner/repo`, `owner/repo@skill`, a git URL, or a local folder, so that I can collect skills from anywhere.
- US-2 (P2) I want to search skills.sh in the app, preview a skill, and add it with one click.
- US-3 (P1) I want skills already in `~/.agents/skills` (from npx skills or by hand) to show up, and to adopt them without losing anything.

Activation
- US-4 (P1) I want to turn a skill on or off globally with one keystroke and see immediately which agents will load it.
- US-5 (P1) I want to create a "Rust" set (skills + a docs MCP server + a fmt hook + a reviewer subagent) and use it in several projects. Changing the set should update all of those projects automatically.
- US-6 (P1) I want to see at a glance which agents will load a skill, including a clear "not supported" for agents that don't read `.agents/skills` (Claude Code today).
- US-7 (P3) I want a project's skills and agent config committed to the repo by default, with a `skills-lock.json` teammates can restore from. Edits teammates make must never be silently overwritten.

Other resources
- US-8 (P1) I want to define an MCP server once and enable it for Claude Code, Codex and Cursor from the matrix. My API keys should stay out of git.
- US-9 (P1) I want to see MCP servers I configured by hand in each agent and adopt them.
- US-10 (P1) I want to define a hook (e.g. format on edit) once and enable it per agent. The app should tell me where an event isn't supported.
- US-11 (P1) I want to keep subagent definitions in one place and have them rendered for each agent.
- US-11b (P1) I want to enable MCP servers, hooks and subagents per project (not only globally). Secrets must never end up in committed project files.
- US-12 (P1) I want to see the plugins each agent has, enable or disable them, and install a plugin from a git repo for Claude Code and Codex.

Updates and safety
- US-13 (P2) I want to be told when an upstream skill changed and see a diff before anything changes, so that my local edits survive.
- US-14 (P1) I want to undo the last change, and I want anything removed or replaced to be recoverable from Trash.
- US-15 (P1) I want the app to refuse to overwrite files it didn't create, and to tell me why.

Sync and CLI
- US-16 (P1) I want to back up my library to a private git repo and get the same setup on my second Mac.
- US-17 (P1) I want every action available as `skm …` with `--json` and `--dry-run`, so that I can script it and put it in my dotfiles.
- US-18 (P1) I want `skm doctor` to explain what's wrong (missing git, unparsable agent config, broken links).

## 6. Functional requirements

Requirements are numbered FR-x and grouped by area. Priority: **M** (must, v1), **S** (should, v1 if time), **L** (later).

| ID | Requirement | P | Stories |
|---|---|---|---|
| FR-1 | Data dir `~/.skillsmgr`, overridable via `SKILLSMGR_HOME` / `--home`; the resolved path is shown in UI and CLI | M | all |
| FR-2 | Agent registry as data; detect the 6 v1 agents; user path overrides | M | US-4,6 |
| FR-3 | Add skills from owner/repo, owner/repo@skill, git URL (+ref, +path), local folder, with a preview before install | M | US-1 |
| FR-4 | skills.sh search + add | M | US-2 |
| FR-5 | Detect foreign entries in `~/.agents/skills`, project `.agents/skills` and agent configs (user + project); adopt with provenance from npx locks | M | US-3,9 |
| FR-6 | Global on/off for skills via `~/.agents/skills` | M | US-4 |
| FR-7 | Matrix shows per-agent skill visibility, incl. "not supported" with reason and issue links for Claude Code | M | US-6 |
| FR-8 | Register projects; direct project items; sets (skills, MCP, hooks, subagents) used by projects as a live link with auto-sync; global duplicates subtracted; pause per project; never commits | M | US-5 |
| FR-9 | Project mode **copy (default)**: committed copies + npx-compatible `skills-lock.json`, rewritten by auto-sync; Edited detection. Optional **link** mode: symlinks + `.git/info/exclude` | M | US-7 |
| FR-10 | MCP: canonical definitions, per-agent render for 6 agents at Global and Project scope, surgical edits, Keychain secrets (Global) / env references only (Project), adopt existing | M | US-8,9,11b |
| FR-11 | Hooks: canonical definitions, event map, render for Claude/Codex/Cursor/Gemini/Copilot at Global and Project scope, unsupported shown | M | US-10,11b |
| FR-12 | Subagents: canonical MD, render for Claude/Codex/Gemini/OpenCode/Copilot (+Cursor rule) at Global and Project scope | M | US-11,11b |
| FR-13 | Plugins: inventory (Claude, Codex, Cursor), enable/disable (Claude, Codex), install via local marketplace | S | US-12 |
| FR-14 | Matrix main view with scope switcher, kind tabs, filter, keyboard control | M | US-4 |
| FR-15 | Update check per source repo; notify; 3-way diff review; apply on approval; ignore version | M | US-13 |
| FR-16 | Ownership ledger; plan preview; trash; undo of the last 50 ops | M | US-14,15 |
| FR-17 | Git sync of the library: init/connect, auto commit/fetch/merge/push, conflict resolver, secret guard | M | US-16 |
| FR-18 | CLI with the full surface in spec §16, `--json`, `--dry-run`, stable exit codes | M | US-17 |
| FR-19 | `skm doctor` with checks and `--bundle` | M | US-18 |
| FR-20 | First-run onboarding: detected agents (with the Claude Code skills note) → adopt existing skills/MCP → done | M | US-3,6 |
| FR-26 | Shared project files (`.mcp.json`, `.claude/agents`, `.codex/agents`) handled without double writes; "via …" visibility; per-agent trust hints | M | US-11b |
| FR-22 | skills.sh leaderboard/curated (needs API access) | L | US-2 |
| FR-23 | Linux and Windows builds | L | — |
| FR-24 | MCP server exposing the core to agents | L | — |
| FR-25 | Per-agent skill opt-out (Codex `skills.config` etc.) | L | — |

## 7. Non-functional requirements

- NFR-1 Performance: meet the budgets in spec §14 (first paint ≤ 300 ms; external change → UI ≤ 500 ms; 120 fps matrix scrolling at 5k rows; ~0% idle CPU; no polling).
- NFR-2 Safety:
  - no literal secret values in any project file; the app never stages or commits in project repos;
  - zero writes to targets not in the ledger without explicit per-action consent;
  - every removal goes to trash (30-day retention);
  - all writes are atomic.
- NFR-3 Correctness: apply is idempotent (re-planning after apply yields an empty plan); an unparsable agent config is never written to.
- NFR-4 Privacy: no telemetry; network only to git remotes the user configured and skills.sh search; secrets only in the Keychain and in the agents' own configs.
- NFR-5 Resilience: the app works offline (everything except search, update checks and sync). Corrupting or deleting `state.db` is recoverable via reindex without data loss.
- NFR-6 Portability: `skm-core` builds and passes its tests on Linux in CI from day one, even though only macOS ships.
- NFR-7 Accessibility: full keyboard operation; state conveyed by glyph as well as color; AccessKit labels where gpui-component supports them.
- NFR-8 Distribution: a signed and notarized `.app` (DMG), plus Homebrew cask (app) and formula (CLI).

## 8. Success metrics

Measured on Sander's machines plus beta users. There is no telemetry, so these are measured via the bench suite and manual checks.
- Cold start to interactive ≤ 300 ms with 1,000 skills (bench, p95).
- 0 data-loss incidents across the beta (Trash + ledger audit).
- Time to make a new Rust project ready (skills + MCP + hooks + subagents): ≤ 10 s (register project → use "Rust" set), producing a clean, committable diff.
- Second-machine setup from sync: ≤ 2 min to an identical global state (excluding secret entry).
- Sander uses it daily for 2 weeks without falling back to manual symlinking or editing agent configs.

## 9. Release scope

**v1 (macOS):** FR-1 to FR-20 and FR-26, including project scope for MCP, hooks and subagents, and copy mode as the default. If FR-13 (plugins) is behind schedule, it ships in 1.1 without blocking v1.
**v1.1:** leftovers of FR-13.
**v2:** FR-22 to FR-25, Linux/Windows, more registry agents (Windsurf, Antigravity, Kiro, …), an `.agents/agents` target if it becomes standard, and Claude Code skills support as soon as Anthropic ships `.agents/skills` (a registry data change).

## 10. Risks and open questions

### Risks

| Risk | Mitigation |
|---|---|
| **Claude Code doesn't read `.agents/skills`** ([#31005](https://github.com/anthropics/claude-code/issues/31005), [#16345](https://github.com/anthropics/claude-code/issues/16345)), so Claude users won't see managed skills | Accepted (no workaround, per Sander). Shown clearly in the matrix; enabling is a registry change once Claude ships support |
| Auto-sync rewrites committed files in many repos at once (unexpected diffs) | Working-tree only, never commits; Activity + Undo; per-project pause; teammate edits detected as Edited, never overwritten |
| Agent trust gates (Codex trusted projects, Copilot folder trust, Claude `.mcp.json` approval, Gemini trusted folders) silently ignore project files we wrote | Per-agent trust hint per project; doctor check where the trust state is readable |
| Shared project files (`.mcp.json` read by Claude + Copilot; `.claude/agents` by Claude + Cursor) | `also_read_by` in the registry, no double writes, "via …" visibility, Copilot MCP written to `.github/mcp.json` |
| Project secrets: env-reference syntax differs and Copilot CLI documents none | Env references only; refuse literal secrets in project files (`E_SECRET_IN_PROJECT`) |
| OpenCode config schema v1 vs v2 (`mcp.<n>` vs `mcp.servers.<n>`) | Detect per file/version; golden tests for both |
| Other tools (npx skills, APM) write `~/.agents/skills` and `skills-lock.json` too | Foreign detection + adopt; lock regenerated only for entries we own, foreign entries preserved |
| gpui-kit snapshot churn | Exact pin + thin UI wrapper; spike first |

### Open questions

None. All questions were answered by Sander on 31 Oct 2026. The resulting decisions are in §11 and in spec §3.

## 11. Final decisions (from Sander's answers, 31 Oct 2026)

| # | Decision |
|---|---|
| F1 | No Claude Code skills bridge or workaround. Claude won't see managed skills until it supports `.agents/skills` (#31005, #16345) |
| F2 | Skill files live in the `~/.skillsmgr` library and are linked into `~/.agents/skills`. There is no alternative placement mode |
| F3 | MCP servers, hooks and subagents have Project scope in v1 |
| F4 | Default project mode = committed copies + npx-compatible `skills-lock.json`; git-excluded links are optional; auto-sync rewrites copies and the lock |
| F5 | Names: skillsmgr (app), `skm` (CLI), `~/.skillsmgr`, `SKILLSMGR_HOME` |
| F6 | skills.sh search only; no leaderboard; no outreach |
| F7 | Synced data lives in `library.toml`; SQLite holds only the index and per-machine state |
| F8 | Foreign skills in `~/.agents/skills` are adopted only on an explicit Adopt |
| F9 | Global MCP secrets are kept in the macOS Keychain and written as real values into user-level configs; project files get env references only |
| F10 | v1 plugins: list, enable/disable and install for Claude Code and Codex; Global only |
| F11 | System git is required (Xcode CLT) |
| F12 | Existing npx `skills-lock.json` entries are merged and left untouched |
| F13 | Sets can include skills, MCP servers, hooks and subagents (plugins stay global-only) |
