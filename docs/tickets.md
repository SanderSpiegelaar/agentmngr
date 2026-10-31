# Tickets

Status: draft v2 (after Sander's answers) · 31 Oct 2026 (Europe/Amsterdam) · The same data is in `tickets.csv`. Tickets below are listed in a valid build order: every dependency appears earlier.

Sizes:

- **S** ≤ 1 day
- **M** 2–3 days
- **L** 4–5 days

Total: 62 tickets (22 S, 35 M, 5 L).

## M0: Foundations and spikes

- [T-001 · Cargo workspace + CI skeleton  `[S]`](tickets/T-001.md)
- [T-002 · Paths, config.toml, SKILLSMGR_HOME  `[S]`](tickets/T-002.md)
- [T-003 · Spike: gpui-kit virtualized matrix  `[M]`](tickets/T-003.md)
- [T-004 · Error type, logging, events skeleton  `[S]`](tickets/T-004.md)

## M1: Core skills engine + CLI

- [T-010 · SQLite store + migrations  `[M]`](tickets/T-010.md)
- [T-011 · library.toml model + read/write  `[M]`](tickets/T-011.md)
- [T-012 · Agent registry (data) + detection  `[M]`](tickets/T-012.md)
- [T-013 · Frontmatter parser + skill validation  `[S]`](tickets/T-013.md)
- [T-014 · Content hash (npx computedHash compatible)  `[S]`](tickets/T-014.md)
- [T-015 · Indexer + stamps  `[M]`](tickets/T-015.md)
- [T-016 · Target classification  `[S]`](tickets/T-016.md)
- [T-017 · Engine: plan/apply, ledger, trash, lock  `[L]`](tickets/T-017.md)
- [T-018 · Undo  `[S]`](tickets/T-018.md)
- [T-019 · Global skills on/off  `[S]`](tickets/T-019.md)
- [T-020 · Foreign detection + adopt (+npx lock provenance)  `[M]`](tickets/T-020.md)
- [T-021 · Watcher  `[M]`](tickets/T-021.md)
- [T-022 · Source spec parser  `[S]`](tickets/T-022.md)
- [T-023 · Git runner (system git)  `[S]`](tickets/T-023.md)
- [T-024 · Install flow (git + local)  `[M]`](tickets/T-024.md)
- [T-025 · skills.sh search client  `[S]`](tickets/T-025.md)
- [T-026 · Projects + multi-kind sets model + effective-set calc  `[M]`](tickets/T-026.md)
- [T-027 · Project copy mode (default) + skills-lock.json  `[M]`](tickets/T-027.md)
- [T-028 · Project auto-sync + Edited safety + pause  `[M]`](tickets/T-028.md)
- [T-029 · Project link mode (optional) + .git/info/exclude  `[S]`](tickets/T-029.md)
- [T-030 · Re-attach existing project content by hash  `[S]`](tickets/T-030.md)
- [T-031 · CLI v1 commands  `[L]`](tickets/T-031.md)
- [T-032 · skm doctor  `[S]`](tickets/T-032.md)

## M2: GPUI app MVP (skills)

- [T-040 · App shell, theme tokens, core→UI event channel  `[M]`](tickets/T-040.md)
- [T-041 · Skills matrix view  `[L]`](tickets/T-041.md)
- [T-042 · Keyboard navigation + command palette  `[M]`](tickets/T-042.md)
- [T-043 · Detail pane  `[M]`](tickets/T-043.md)
- [T-044 · Discover sheet (search, URL, local)  `[M]`](tickets/T-044.md)
- [T-045 · Projects and Sets views  `[M]`](tickets/T-045.md)
- [T-046 · Activity view + Undo  `[S]`](tickets/T-046.md)
- [T-047 · Foreign/conflict UX  `[S]`](tickets/T-047.md)
- [T-048 · First-run onboarding  `[M]`](tickets/T-048.md)
- [T-049 · Design review pass  `[S]`](tickets/T-049.md)

## M3: MCP, hooks, subagents, plugins

- [T-060 · Config edit layer (user + project files)  `[L]`](tickets/T-060.md)
- [T-061 · Keychain secrets + project env references  `[S]`](tickets/T-061.md)
- [T-062 · Shared-file planner + trust hints  `[M]`](tickets/T-062.md)
- [T-063 · MCP canonical model + 6 renderers (Global + Project)  `[L]`](tickets/T-063.md)
- [T-064 · MCP adopt existing servers (user + project)  `[M]`](tickets/T-064.md)
- [T-065 · Subagents: canonical + renderers (Global + Project)  `[M]`](tickets/T-065.md)
- [T-066 · Hooks: canonical + event map + renderers (Global + Project)  `[M]`](tickets/T-066.md)
- [T-067 · Sets + project auto-sync for MCP, hooks, subagents  `[M]`](tickets/T-067.md)
- [T-068 · Plugins: inventory + enable/disable (Global)  `[M]`](tickets/T-068.md)
- [T-069 · Plugins: local marketplace install  `[M]`](tickets/T-069.md)
- [T-070 · Matrix tabs for MCP/hooks/subagents/plugins (both scopes)  `[M]`](tickets/T-070.md)
- [T-071 · CLI for other kinds  `[S]`](tickets/T-071.md)

## M4: Updates

- [T-080 · Update checker  `[M]`](tickets/T-080.md)
- [T-081 · 3-way update apply (+ project propagation)  `[M]`](tickets/T-081.md)
- [T-082 · Updates UI + notifications  `[M]`](tickets/T-082.md)

## M5: Git sync

- [T-090 · Sync init/connect  `[M]`](tickets/T-090.md)
- [T-091 · Auto-sync loop  `[M]`](tickets/T-091.md)
- [T-092 · Conflict resolver (CLI + GUI)  `[M]`](tickets/T-092.md)
- [T-093 · Secret guard + missing-secret prompts  `[S]`](tickets/T-093.md)
- [T-094 · Large-file limits  `[S]`](tickets/T-094.md)

## M6: Hardening and release

- [T-100 · Bench suite + CI regression gate  `[M]`](tickets/T-100.md)
- [T-101 · Perf pass against budgets  `[M]`](tickets/T-101.md)
- [T-102 · Packaging: sign, notarize, DMG, Homebrew  `[M]`](tickets/T-102.md)
- [T-103 · Docs  `[S]`](tickets/T-103.md)
- [T-104 · Dogfood + bug bash  `[M]`](tickets/T-104.md)
