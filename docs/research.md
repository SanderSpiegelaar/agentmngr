# Research: rebuilding Skills Manager in Rust + GPUI

Status: planning input · Researched 31 Oct 2026 (Europe/Amsterdam) · Companion to `../skills-manager-analysis.md` (analysis of upstream v1.43.2, HEAD f9479c3)

Each section ends with a **Verdict** that feeds `spec.md`. Where a finding contradicts one of Sander's assumptions, it is marked **⚠ Contradiction**.

---

## 1. Agent Skills format (the unit we manage)

Source: <https://agentskills.io/specification>

- A skill is a directory containing `SKILL.md`, which has YAML frontmatter and a Markdown body. Optional subfolders are `scripts/`, `references/` and `assets/`, plus any other files.
- Required frontmatter:
  - `name`: 1–64 chars; `a-z`, `0-9` and `-` only; no leading, trailing or double `-`; **must equal the parent directory name**.
  - `description`: 1–1024 chars.
- Optional frontmatter:
  - `license`;
  - `compatibility` (≤500 chars);
  - `metadata` (string→string map);
  - `allowed-tools` (experimental).
- Progressive disclosure: only name and description (~100 tokens) are loaded at agent startup. The body is loaded on activation. Codex caps the skill list at about 2% of context (developers.openai.com/codex/skills). The *number* of globally active skills therefore matters to users, which is a product argument for sets and scopes.
- Codex reads an optional `agents/openai.yaml` sidecar for UI metadata. Treat it as an opaque file.

**Verdict:** Validate against the spec (name ↔ dir mismatch is a warning, not a blocker, because many real skills violate it). Index only frontmatter at scan time. Never parse bodies on the hot path.

## 2. Who reads `~/.agents/skills` and `.agents/skills`

Sources:
- <https://developers.openai.com/codex/skills>
- <https://cursor.com/docs/skills>
- vercel-labs/skills `docs/agents.md` and `src/agents.ts` (cloned at /tmp/vskills)
- APM targets matrix <https://microsoft.github.io/apm/reference/targets-matrix/> (updated 22 Oct 2026)
- <https://code.claude.com/docs/en/skills>

| Agent | User scope reads `~/.agents/skills` | Project reads `.agents/skills` | Native dir(s) |
|---|---|---|---|
| Codex | **yes (only documented user path)** + `/etc/codex/skills` | yes, walks CWD→repo root | (legacy `~/.codex/skills`) |
| Cursor | yes | yes | also `~/.cursor/skills`, `~/.claude/skills`, `~/.codex/skills` → duplicates possible |
| GitHub Copilot (CLI/VS Code) | yes | yes | `.github/skills` |
| Gemini CLI | yes | yes | `.gemini/skills` |
| OpenCode | yes | yes | `.opencode/skills`, also reads `.claude/skills` |
| Windsurf, Cline, Warp, Zed, Pi, Kimi, Dexto, Rovo Dev | yes | mostly | various |
| Antigravity, Droid, Kilo, Firebender, Deep Agents | — / partial | yes | various |
| **Claude Code** | **no** | **no** | `~/.claude/skills`, `.claude/skills` (walks parents), managed dir, plugin skills |
| Grok, Kiro, Hermes (user scope) | no | partial | native dirs |

**⚠ Contradiction (high impact).** Claude Code does **not** read `~/.agents/skills` or `.agents/skills`:
- The official docs (code.claude.com/docs/en/skills) list only `~/.claude/skills`, `.claude/skills`, plugin skills and managed policy dirs. A text search of the page finds no `.agents`.
- Open feature requests: anthropics/claude-code#31005 and #16345. Both are still open and unresolved as of Oct 2026.
- Cursor reads both `~/.claude/skills` and `~/.agents/skills`. Bridging the two by links would risk Cursor loading every skill twice (upstream bug "duplicate loading in Cursor").

**Verdict (Sander's decision, 31 Oct 2026):** `.agents/skills` is the only skills target. **There is no Claude bridge and no workaround.** Claude Code won't see managed skills until it supports `.agents/skills`; we track #31005 and #16345. The matrix shows Claude Code's skills column as "not supported (reads only `~/.claude/skills`)". The registry entry is pure data, so when Claude adds support, enabling it is a one-line change.

## 3. Ecosystem tools that also write `~/.agents/skills`: npx skills (vercel-labs/skills)

Sources: github.com/vercel-labs/skills `src/skill-lock.ts`, `src/local-lock.ts`, `src/find.ts`, `src/installer.ts`, `docs/agents.md`.

- Canonical dir is `~/.agents/skills`. "Universal" agents read it directly; other agents get symlinks or copies (`installMode` symlink | copy | link).
- **Global lock** at `~/.agents/.skill-lock.json` (or `$XDG_STATE_HOME/skills/.skill-lock.json`), `version: 3`. It contains `skills: { <name>: {...} }` with these fields:
  - `source`, `sourceType` (github | gitlab | git | local | well-known | …), `sourceUrl`, `ref?`, `skillPath?`;
  - `skillFolderHash` (GitHub tree SHA of the skill folder);
  - `installedAt`, `updatedAt`, `pluginName?`, `sourceBaseUrl?`, `wellKnownDigest?`, `installMode?`.

  The lock also has `dismissed` and `lastSelectedAgents`.
- **Project lock** at `skills-lock.json` in the repo root, `version: 1`. It contains `skills: { <name>: { source, sourceUrl?, ref?, sourceType, skillPath?, version?, via?, computedHash, subagents?, wellKnownDigest?, installMode? } }`. Keys are sorted and local sources are written relative (`./…`).
  - `computedHash` = SHA-256 over all files sorted by relative path, feeding `relPath` then the file bytes.
  - Restore with `npx skills experimental_install` (`--frozen-lockfile`).
- Search uses the unauthenticated `https://skills.sh/api/search?q=…&limit=…` (overridable with `SKILLS_API_URL`). Downloads use `skills.sh/api/download`.

**⚠ Consequence of delivering into `~/.agents/skills`.** Library content lives in `~/.skillsmgr/library` and is linked into `~/.agents/skills`, but other tools write there too: npx skills, APM, agents' own skill-creator flows, and hand edits. The app can never assume exclusive ownership of that directory. It must:
1. treat unknown entries as *unmanaged but visible* and offer **Adopt**;
2. read `.skill-lock.json` to attribute sources on adopt;
3. never delete or overwrite anything it didn't write (ownership ledger, §spec 7.2).

**Verdict:** Interop with npx skills:
- v1 reads both lock formats (to adopt existing installs with their provenance).
- v1 **writes project `skills-lock.json` by default**, because the default project mode is committed copies. Teammates without the app can restore with `npx skills experimental_install`.
- Writing the global `.skill-lock.json` is deferred: it is npx's file and two writers will race.
- Reuse the `computedHash` algorithm as our content hash so hashes are comparable.

## 4. skills.sh

Sources: <https://skills.sh/docs/api>, live tests on 31 Oct 2026, and vercel-labs/skills `src/find.ts`.

- The official v1 API covers:
  - `GET /api/v1/skills` (leaderboard; `view=all-time|trending|hot`, `page`, `per_page`, `source`);
  - `/api/v1/skills/search`;
  - `/api/v1/skills/curated`;
  - `/api/v1/skills/{source}/{skill}` (files and hash);
  - `/api/v1/audit/...`.
- **The v1 API requires a Vercel OIDC token.** Unauthenticated calls return `401` (tested). A distributed desktop app cannot obtain one.
- The legacy `GET https://skills.sh/api/search?q=<q>&limit=<n>` works unauthenticated (`200`). It returns `{id, source, skillId, name, installs}`. This is exactly what `npx skills find` uses.
- There is no public unauthenticated leaderboard or trending endpoint. The only options are HTML scraping (fragile, ToS risk) or private API access.

**⚠ Risk.** "Browse skills.sh" in v1 = **search** (plus "popular" via a broad query ordered by `installs`) using the same endpoint as the official CLI. A leaderboard or curated view is **out of scope** (Sander, 31 Oct 2026: search-only, no outreach to Vercel).

**Verdict:** The skills.sh client is a thin, swappable module with base-URL override (`SKILLSMGR_SKILLS_API`, honoring `SKILLS_API_URL`). Install always goes through git (source `owner/repo` plus skill path). We don't depend on skills.sh for downloads.

## 5. Other resource types: real locations and formats

### 5.1 MCP servers

Sources:
- code.claude.com/docs/en/mcp
- developers.openai.com/codex/config-reference
- cursor.com/docs/context/mcp
- geminicli.com/docs/tools/mcp-server
- opencode.ai/docs/mcp-servers
- docs.github.com Copilot CLI MCP
- agent-plugins.org/specification
- APM targets matrix

| Agent | User scope | Project scope | Shape |
|---|---|---|---|
| Claude Code | `~/.claude.json` → top-level `mcpServers` (local scope: `projects["<abs path>"].mcpServers`) | `.mcp.json` | `mcpServers{name:{type: stdio\|http\|sse, command, args, env, url, headers}}`; `${VAR}` expansion; respects `CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/config.toml` | `.codex/config.toml` (trusted projects) | `[mcp_servers.<name>] command,args,env,env_vars,url,bearer_token_env_var,enabled_tools,enabled` |
| Cursor | `~/.cursor/mcp.json` | `.cursor/mcp.json` | `mcpServers` (command/args/env or url/headers; `${env:VAR}`) |
| Gemini CLI | `~/.gemini/settings.json` | `.gemini/settings.json` | `mcpServers{command,args,env,cwd,url(SSE),httpUrl,headers,timeout,trust}`; avoid `_` in names |
| OpenCode | `~/.config/opencode/opencode.json` | `opencode.json` | `mcp{name:{type: local, command:[…], environment} \| {type: remote, url, headers}, enabled}` |
| Copilot CLI | `~/.copilot/mcp-config.json` (`COPILOT_HOME`) | `.mcp.json` / `.github/mcp.json` (VS Code: `.vscode/mcp.json` with `servers`) | `mcpServers{type: local\|http, command,args,env,url,tools}` |
| Antigravity | `~/.gemini/config/mcp_config.json` | `.agents/mcp_config.json` | `mcpServers` |
| Kiro | `~/.kiro/settings/mcp.json` | `.kiro/settings/mcp.json` | `mcpServers` |

- The cross-agent standard closest to all of these is the **`mcpServers` JSON object** as formalised in the Agent Plugins `mcp.json` (`type: stdio | streamable-http`, `command`, `args`, `env`, `url`, `headers`).
- `~/.claude.json` is a large, hot state file that Claude Code rewrites while running. Editing it directly races with a running Claude, so prefer `claude mcp add-json -s user` and `claude mcp remove` when the CLI is installed.
- Secrets usually live in `env` or `headers`. Expansion syntax differs per agent (`${VAR}`, `${env:VAR}`, `$VAR`, `env_vars`), and GUI-launched agents don't inherit shell env.

**Verdict:**
- Canonical definition = one JSON file per server in the Agent Plugins `mcpServers` entry shape.
- Render per agent with surgical edits:
  - `toml_edit` for Codex (preserves comments and order);
  - `serde_json` with `preserve_order` for JSON;
  - a JSONC-aware edit for OpenCode `.jsonc`;
  - Claude's CLI for `~/.claude.json`.
- Track ownership per entry. Secrets are stored in the macOS Keychain and never committed to sync.

### 5.2 Hooks

Sources:
- code.claude.com/docs/en/hooks
- developers.openai.com/codex (hooks)
- cursor.com/docs/agent/hooks
- geminicli.com docs (hooks)
- docs.github.com/en/copilot/reference/hooks-reference

| Agent | Where | Shape / events |
|---|---|---|
| Claude Code | `hooks` key in `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json` | `{Event:[{matcher, hooks:[{type:"command", command, timeout}]}]}`; PreToolUse, PostToolUse, UserPromptSubmit, SessionStart, Stop, SubagentStop, PreCompact, Notification, … |
| Codex | `~/.codex/hooks.json`, `.codex/hooks.json`, or `[hooks]` in config.toml (layers merged; needs `features.hooks`) | Same nesting as Claude; PreToolUse, PermissionRequest, PostToolUse, Pre/PostCompact, SessionStart, SubagentStart/Stop, UserPromptSubmit, Stop; command hooks only |
| Cursor | `~/.cursor/hooks.json`, `.cursor/hooks.json` | `{"version":1,"hooks":{"afterFileEdit":[{"command":…}]}}`; camelCase events (preToolUse, beforeShellExecution, sessionStart, stop, …) |
| Gemini CLI | `hooks` in settings.json | BeforeTool, AfterTool, BeforeAgent, SessionStart, …; `{matcher, hooks:[{type:"command",command}]}` |
| Copilot CLI | `~/.copilot/hooks/*.json`, `.github/hooks/*.json` | `{"version":1,"hooks":{"sessionStart":[{"type":"command","bash":…,"timeoutSec":…}]}}`; **separate files allowed** |
| OpenCode | none (JS plugins only) | — |

There is **no cross-agent hook standard**. Claude and Codex share a schema. Cursor, Gemini and Copilot need event-name mapping, and some events have no equivalent.

**Verdict:**
- Canonical form = the Claude/Codex schema (one file per hook).
- Use a per-agent event-map table in the registry. Unmappable events are skipped and shown as "not supported" in the matrix (lossy by design, visible to the user).
- Hooks execute code, so enabling one always shows the exact command first.

### 5.3 Subagents

Sources:
- code.claude.com/docs/en/sub-agents
- cursor.com/docs (subagents)
- developers.openai.com/codex (agents)
- geminicli.com/docs/core/subagents
- opencode.ai/docs/agents
- docs.github.com (custom agents)
- antigravity.google/docs/subagents
- docs.gitlab.com (workspace subagents)

| Agent | User | Project | Format |
|---|---|---|---|
| Claude Code | `~/.claude/agents/*.md` | `.claude/agents/*.md` (recursive) | MD + YAML: name, description, tools, disallowedTools, model, permissionMode, mcpServers, hooks, skills, … |
| Cursor | `~/.cursor/agents/` | `.cursor/agents/` | MD; **also reads `.claude/agents` and `.codex/agents`** |
| Codex | `~/.codex/agents/*.toml` | `.codex/agents/*.toml` | TOML: name, description, developer_instructions, model, … |
| Gemini CLI | `~/.gemini/agents/*.md` | `.gemini/agents/*.md` | MD: name, description, kind, tools, model, temperature, max_turns, timeout_mins |
| OpenCode | `~/.config/opencode/agents/` | `.opencode/agents/` | MD |
| Copilot | `~/.copilot/agents/*.agent.md` | `.github/agents/*.agent.md` | MD |
| Antigravity, GitLab Duo, Pi | — / Pi global | **`.agents/agents/*.md`** (emerging shared path) | MD |

**Verdict:**
- Canonical = Markdown with Claude-style frontmatter (de-facto common).
- Render a copy per agent, converting to TOML for Codex and dropping unknown keys per agent.
- Because Cursor reads `.claude/agents`, don't also write `.cursor/agents` when Claude is enabled (avoids duplicates).
- Watch `.agents/agents` as the likely future universal target. Add it as a registry entry when ≥2 major agents read it.

### 5.4 Plugins

Sources:
- agent-plugins.org/specification (Agent Plugins 1.0.0; spec copy at /tmp/ap.md)
- code.claude.com/docs/en/plugins and plugin-marketplaces
- developers.openai.com/plugins/build/plugins
- cursor.com/docs/reference/plugins
- APM targets matrix

- **Agent Plugins 1.0.0** (open standard):
  - root `plugin.json`, `skills/<name>/SKILL.md` and root `mcp.json`;
  - `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` variables;
  - example install root `~/.agents/plugins/<name>`, data in `~/.agents/plugins/data/<name>`.
- Claude Code:
  - manifest `.claude-plugin/plugin.json`; marketplaces via `.claude-plugin/marketplace.json`;
  - state in `~/.claude/plugins/{installed_plugins.json, known_marketplaces.json, cache/, data/, marketplaces/}`;
  - enablement via `enabledPlugins {"name@mkt": true}` and `extraKnownMarketplaces` in settings.json;
  - directory marketplaces load in place;
  - CLI: `claude plugin marketplace add`, `claude plugin install|enable|disable`.
- Codex:
  - reads root `plugin.json` (Agent Plugins), with `.codex-plugin/plugin.json` as fallback;
  - personal marketplace `~/.agents/plugins/marketplace.json` and repo marketplace `.agents/plugins/marketplace.json` (also reads `.claude-plugin/marketplace.json`);
  - installs to `~/.codex/plugins/cache/<mkt>/<plugin>/<ver>/`;
  - enablement via `[plugins."name@mkt"] enabled = true`;
  - CLI: `codex plugin marketplace add <path>`.
- Cursor: Agent Plugins plus `.cursor-plugin/plugin.json`.
- Gemini: extensions (`gemini-extension.json`).
- OpenCode: JS/TS plugins (different concept).

**Verdict:** Plugins are the most agent-specific area and the highest risk.
- v1 = **inventory** (read each agent's installed and enabled state) + **enable/disable** + **install from a git source via an app-hosted local marketplace**, driving each agent's own CLI (`claude plugin …`, `codex plugin …`) rather than editing their state files.
- Translation between plugin formats is a non-goal.

## 5.5 Project scope for MCP, hooks and subagents (v1 requirement)

Sources:
- code.claude.com/docs/en/mcp, /settings, /hooks, /sub-agents
- developers.openai.com/codex/config-reference, learn.chatgpt.com/docs/config-file/config-basic and /docs/hooks
- cursor.com/docs (mcp, hooks, subagents)
- geminicli.com/docs (configuration, trusted folders, subagents)
- opencode.ai/docs/config, /mcp-servers, /agents and opencode.ai/v2/docs/config
- docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers, hooks-reference, custom agents
- github/copilot-cli#3019 (`.vscode/mcp.json` dropped)

| Agent | Project MCP | Project hooks | Project subagents | Trust gate / notes |
|---|---|---|---|---|
| Claude Code | `.mcp.json` (repo root; `mcpServers`). Personal per-project: "local" scope in `~/.claude.json` `projects["<abs>"]` | `.claude/settings.json` (shared) or `.claude/settings.local.json` (personal; Claude git-ignores it) under `hooks` | `.claude/agents/*.md` | Asks the user to approve servers from `.mcp.json` on first use |
| Codex | `.codex/config.toml` `[mcp_servers.<name>]` (closest to CWD wins) | `.codex/hooks.json` (or `[hooks]` in `.codex/config.toml`) | `.codex/agents/*.toml` | **Project `.codex/` layers load only for trusted projects.** Hooks also need per-hash hook trust |
| Cursor | `.cursor/mcp.json` | `.cursor/hooks.json` (`version: 1`) | `.cursor/agents/*.md` (also reads `.claude/agents`, `.codex/agents`) | — |
| Gemini CLI | `.gemini/settings.json` `mcpServers` | `.gemini/settings.json` `hooks` | `.gemini/agents/*.md` | Trusted-folders feature can skip project settings |
| OpenCode | `opencode.json(c)` at repo root or `.opencode/opencode.json(c)`; v1 schema `mcp.<name>`, **v2 schema `mcp.servers.<name>` with `disabled`** | — (no hooks) | `.opencode/agents/*.md` | `.opencode/` configs override root configs |
| Copilot CLI | `.github/mcp.json` (committed, shared) or `.mcp.json` (walks CWD→repo root; `.mcp.json` wins in the same dir); `.vscode/mcp.json` is **not** read since v1.0.22 | `.github/hooks/*.json` (separate files allowed) | `.github/agents/*.agent.md` | Project MCP loads only after folder trust |

Findings that shape the design:
1. **Shared files.** `.mcp.json` is read by both Claude Code and Copilot CLI. `.claude/agents` is read by Claude Code and Cursor. Writing for one agent therefore also affects another. The registry needs a `also_read_by` notion, and the matrix must show "via .mcp.json" / "via .claude/agents" rather than a misleading Off.
2. **Choose files that don't collide.** For Copilot, write `.github/mcp.json` rather than `.mcp.json`, so Claude's dialect (`type: stdio`) and Copilot's (`type: local`) never share an entry. Copilot still also sees Claude's `.mcp.json` entries; that is shown in the matrix.
3. **Project files are usually committed**, so literal secrets must never be written there. Every dialect except Copilot documents env interpolation:
   - Claude `${VAR}`;
   - Cursor `${env:VAR}`;
   - Codex `env_vars` / `bearer_token_env_var`;
   - Gemini `$VAR` / `${VAR}`;
   - OpenCode `{env:VAR}`.

   The renderer must emit env references at project scope and refuse when a dialect can't express one.
4. **Trust gates.** Codex, Copilot, Claude (.mcp.json approval) and Gemini (trusted folders) may silently ignore files we write until the user trusts the project in that agent. We can't and shouldn't set trust; we show a per-agent hint.
5. **Personal vs shared.** Only Claude offers a personal project file (`.claude/settings.local.json`, plus local MCP scope in `~/.claude.json`). Everything else is a shared repo file. KISS: v1 always writes the shared project file. In link mode (local), files we created are added to `.git/info/exclude`; files already tracked by git are edited in place, and the user is told.
6. **OpenCode has two schemas** (v1 `mcp.<name>`, v2 `mcp.servers.<name>`). The renderer must detect which one the project's existing file or installed version uses.

**Verdict:** Project scope for MCP, hooks and subagents is feasible in v1 with the same canonical items and renderers as Global. Only the target path table (registry data) and three rules change:
- env-reference-only secrets;
- shared-file awareness;
- trust hints.

## 6. GPUI: current state

Sources:
- crates.io/crates/gpui
- zed discussion #55271
- crates.io/crates/gpui-unofficial
- crates.io/crates/gpui-standalone
- github.com/longbridge/gpui-kit
- crates.io/crates/gpui-component
- github.com/zed-industries/awesome-gpui
- zed.dev/blog/zed-for-windows-is-here

- The official `gpui` on crates.io is still **0.2.2 (22 Oct 2025)**, a year old. The Zed team can't currently publish the live GPUI (#55271), so you either depend on zed git (huge workspace, unpinned churn) or use a snapshot fork.
- Snapshot options:
  - **`gpui-unofficial` 1.26.2**: mirrors Zed v1.26.2 (gpui-archipelago).
  - **`gpui-standalone` 1.4.4**: few users.
  - **longbridge `gpui-kit` 0.8.1 (26 Oct 2026)**: pins `gpui-pre = 0.3.10` (snapshot of zed@08cbb88) and re-exports:
    - GPUI;
    - `gpui-base` (unstyled behaviours);
    - `gpui-component` 0.8.1: 75+ components, including `v_virtual_list` (variable-height virtual list), **DataTable with virtual rows, resizable and fixed columns**, Markdown renderer, code editor (tree-sitter), dock, AccessKit, headless UI tests, `gpui-fast` feature.

    It is used in production by Longbridge Pro (macOS, Windows, Linux).
- Core GPUI also offers `uniform_list` and `list` (virtualized) for hand-rolled grids.
- Platform maturity: macOS is GPUI's home platform (Metal), and Zed ships on it daily. Linux (Vulkan/Blade/wgpu) is stable. Windows (DirectX 11 + DirectWrite) went GA with Zed for Windows. The portable-core plan is realistic.
- Standalone GPUI apps to learn from: Loungy (launcher), Glim, Waku, Zeron, GitComet, zedis, DBFlux, Hummingbird, pgui, Helix-GPUI.
- Known gaps:
  - API churn (breaking changes between snapshots);
  - thin docs;
  - no native menus or tray beyond what Zed needs;
  - accessibility via AccessKit only in gpui-component;
  - packaging, signing and notarization are DIY (cargo-bundle / cargo-packager).

**Verdict:**
- Depend on **`gpui-kit` (pinned exact version)** for GPUI + DataTable + virtual lists. Use `gpui-base` primitives plus our own theme tokens to avoid the stock "component library" look.
- Wrap GPUI usage behind a thin `ui` module so a later move to another snapshot is mechanical.
- Spike first (T-003): a 5,000-row × 12-column virtualized matrix at 120 fps on macOS.

## 7. Git: gix vs git2 vs system git

Sources:
- crates.io gix 0.90.0, git2 0.21.0
- gitoxide `crate-status.md`
- upstream issues: SSH and credential failures with libgit2

- **gix 0.90.0**: pure Rust; fast clone/fetch over HTTPS and SSH (via the ssh binary); **no push**. Its own guidance is to shell out to `git` for push.
- **git2 0.21.0** (libgit2): push works, but credentials are DIY: SSH agent, credential helpers, `~/.ssh/config` and 1Password agent all need custom code. That is the root of several upstream bug reports. C dependency (vendored OpenSSL/libssh2 on Linux).
- **System `git` CLI**:
  - honours the user's SSH config, credential helpers, GPG signing and proxies with zero code;
  - present on most dev Macs (Xcode CLT; `/usr/bin/git` triggers the CLT install prompt if missing).
  - Cost: process spawn and output parsing.

**Verdict:** **Shell out to system `git`** for all network operations (clone, fetch, ls-remote, push) and for merges (`git merge-file`, `git merge`), with timeouts and `GIT_TERMINAL_PROMPT=0`. Do not add gix or git2 in v1 (YAGNI). Revisit gix for read-only speedups only if profiling demands it. `doctor` checks for git ≥ 2.30.

## 8. File watching: notify

Sources: crates.io notify 8.2.1, notify-debouncer-full 0.7.0 (docs.rs).

- notify 8 uses FSEvents on macOS (recursive watches are cheap, kernel-coalesced), inotify on Linux (one watch per dir; the limit `max_user_watches` matters) and ReadDirectoryChangesW on Windows.
- `notify-debouncer-full` coalesces bursts, stitches rename pairs (FSEvents/inotify) and tracks file IDs. A 200–300 ms debounce suits "git checkout wrote 400 files".
- Pitfalls:
  - FSEvents reports at directory granularity under load (needs a "rescan this dir" fallback);
  - the upstream app watched `.git` and `node_modules` and polled every 3 s; never do either.

**Verdict:**
- Use notify + notify-debouncer-full with path-scoped targets (exact list in spec §8).
- Map events to *items* and re-index only those.
- No polling. On startup, catch up with a cheap stamp comparison instead of a full re-hash.

## 9. SQLite: rusqlite vs sqlx

Sources: crates.io rusqlite 0.41.1, sqlx 0.9.0.

- rusqlite: synchronous, tiny, `bundled` feature (no system lib), full SQLite feature access (WAL, `user_version` migrations, `data_version`).
- sqlx: async (tokio-centric) with compile-time checked queries (needs a DB at build time or offline data). GPUI has its own executor and is not tokio, so sqlx would pull in a second runtime for no gain.

**Verdict:**
- Use **rusqlite (bundled) + WAL**: one writer connection owned by a core "store" thread and N read connections.
- Migrations via `PRAGMA user_version` with plain SQL files (`rusqlite_migration` crate optional).

## 10. Other crates (proposed)

- `toml_edit`: comment-preserving TOML edits.
- `serde_json` with `preserve_order`; `jsonc-parser` for `.jsonc`.
- `serde_yaml_ng` (or `yaml-rust2`) for frontmatter. `serde_yaml` is unmaintained.
- `clap` 4 (CLI).
- `thiserror` (errors), `tracing` + `tracing-appender` (logs).
- `sha2` (hashing), `walkdir` + `ignore` (scan with ignore rules), `similar` (diffs for display).
- `fs4` or `fd-lock` (inter-process lock), `ulid`.
- `security-framework` (macOS Keychain) behind a `secrets` trait; `keyring` for portability later.
- `ureq` (blocking HTTP for skills.sh; no tokio).
- `directories` (platform paths).

## 11. Upstream lessons that drive requirements (from the analysis)

| Upstream problem | Requirement in the rebuild |
|---|---|
| Startup blocked on `reindex_from_metadata` (90 s for one 13k-file skill, #623) | Startup reads the SQLite index only; nothing walks file trees before first paint (NFR-1) |
| Watcher → full refresh (4 IPC calls, re-parse every SKILL.md, re-hash project skills) plus 3 s polling, watching `.git` | Item-scoped events → item-scoped reindex → per-row UI update; no polling (§8) |
| Single `Mutex<Connection>`; global lock held during git network operations | Writer thread + readers; lock only around apply, never around network I/O |
| No list virtualization | All lists virtualized (DataTable / virtual list) |
| DB + JSON metadata as dual truth | One owner per fact: files for synced definitions, SQLite for index and machine-local state (§5) |
| 56 hard-coded adapters; Codex deployed to `~/.codex/skills` | Agent registry as data; correct defaults; one skills target |
| `add_project` creates `.claude/skills` and `*-disabled` dirs in repos | Never create agent dirs in repos unless the user enables that project scope; disabling = removing a link, never moving user files |
| Data loss on update (#256), deleting unmanaged dirs (#363) | Ownership ledger + trash + 3-way update + dry-run plans |
| Confusing preset vs enabled vs deployed | Three words only: **Library**, **Set**, **Scope** (spec §2) |
| Webview black screen and IPC jank | Native GPUI; no IPC boundary |

Keep from upstream: the target-classification model (Absent / LinkToUs / ForeignLink / RealDir / RealFile) plus an explicit replace policy.
