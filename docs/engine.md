# Engine API (T-017)

## System Git (T-023)

`git::GitRunner::new(timeout)` checks system Git >=2.30. `with_binary(path, timeout)`
injects an executable for tests. Commands disable terminal prompts, capture stderr,
and enforce the timeout, including output collection. Auth failures return
`E_GIT_AUTH`; transport failures and timeouts return `E_NETWORK`; missing or old
Git returns `E_GIT_MISSING`. Other command failures return `E_IO` with stderr.
These synchronous methods belong on worker threads, not the GUI thread.

`clone_bare(url, destination, optional_branch)` uses a shallow blobless bare clone.
`bare(path)` reopens only bare repositories. `BareRepository` exposes `fetch`,
`ls_tree`, `rev_parse`, and `archive` (tar bytes). Fetch callers resolve
`FETCH_HEAD` for the fetched revision. `project(path)` returns `ProjectRepository`
with only `ls_files` (repository-relative paths) and `exclude_path` (absolute,
worktree-aware). Neither handle exposes arbitrary commands, add, or commit.
Tree and tracked-path lists use NUL delimiters and preserve Unix filename bytes.

## Reconciliation

`engine::observe(&paths, &store, &targets)` reads exact target fingerprints and
whole-path ledger rows. `engine::plan(scope_id, &desired, &observations)` is pure:
only explicitly supplied targets are reconciled. Request `DesiredContent::Absent`
to turn an item off; omitting it does nothing. The scope must already exist in
SQLite. Paths must be absolute, without `..` or symlink ancestors.
Call `Paths::ensure_layout(&mut self)` after resolving the manager home. It creates
the layout and canonicalizes its directories once, before plans or store use.
Restore containment compares canonical backup and trash paths. Link classification
resolves both the link destination and library, including missing item roots.
The injected user home is canonicalized before constructing global targets;
symlinked `.agents` descendants remain refused.

`engine::apply(&paths, &store, &plan, actor, events)` returns completed actions,
refusal issues, failures, and inverse guarded actions in reverse execution order.
It takes a nonblocking fd-lock only during apply. A competing process receives
`E_LOCKED`. Every completed apply, including empty plans and partial batches,
writes an ops row and touches `.changed`; optional events follow lock release.

`Action` contains `Link`, `WriteFile`, `CopyTree`, `Claim`, `Trash`, and `Restore`.
`PlannedAction` pairs the effect with its expected `Observation`, resulting
ownership, and explicit modified-content approval. Apply checks the observation,
ledger, and ownership rule again, even for manually constructed plans. Foreign
targets never become ordinary reconciliation actions. `approve_modified` applies
only to ledger-owned files or trees, not foreign entries. Adoption is the explicit
exception described below.

Repeated identical item/destination requests collapse into one action. Different
content or items at one destination produce an issue instead. Registry-specific
visibility and dialect selection remain the caller's responsibility. Entry-level
config editing and external CLI actions are not implemented here.

Tree sources use `engine::snapshot(source, &paths)?.hash`, **not** npx
`content_hash`: ownership fingerprints include every file, empty directory and
raw symlink destination, including `.git` and `node_modules`. Tree copies preserve
symlinks without following them and retain permissions. Sources are hash-checked
before and after staging. Config renderers can use `WriteFile` only for files
wholly owned by the engine; shared entry-level files need the later surgical
editing implementation.

Replaced content and removed links move to `trash/YYYYMMDD-HHMMSS-<op-id>/` (UTC).
Files and links publish by rename; tree replacements can briefly be absent but
never partially populated. Cross-filesystem trash renames fail safely with
`E_IO`. A killed process may leave hidden `.skm-*.skm-tmp` staging entries, never
partial content at the requested destination. This is per-target atomicity, not
an all-or-nothing transaction over the whole plan or SQLite and the filesystem.

`engine::list_ops(&store)` returns the last 50 operations, newest first.
`engine::undo(&paths, &store, op_id, actor, events)` selects one of those operations
(`None` selects the latest), deserializes its inverses, and reuses apply with the
original scope. Undo itself records an operation and can be undone. Unfinished
`applying` operations, unknown IDs, and empty history refuse with `E_CONFIG_CHANGED`.
Under the apply lock, undo preflights every target's exact post-apply fingerprint
and ledger row and every restore backup before changing any target. A known
conflict refuses the whole undo without adding an ops row. Per-action checks
remain in place; filesystem failures or external edits during execution can still
produce a partial result, as with regular apply. `Restore` copies hash-verified
trash backups, keeping them available. `skm undo [op-id]` runs undo and
`skm undo --list` lists history.

## Foreign skills and adoption (T-020)

`foreign_skills::list_foreign(&paths, &store, user_home, project)` reads universal
skills and returns `ForeignSkill` entries with classification and optional lock
provenance. `project` is an optional project directory; `None` selects Global.
Absent entries, our links, and exact ledger-owned project copies are omitted.
Discovery and `plan_adopt(..., &names, project)` create no target directories,
scopes, operations or metadata. SQLite reads may update its runtime shared-memory
file, not desired state or skill content. CLI initialization can still create the
manager layout, logs and database as it does for other previews.

`foreign_skills::adopt` binds the preview to a real scope and calls engine apply
once. Global adoption plans `CopyTree` into `library/skills/<name>`, then `Link`
at the original target, then `WriteFile` for rendered `library.toml`. `CopyTree`
checks both the exact engine fingerprint and its optional npx `computed_hash`
on staged content before publishing. `Link` moves an original directory into
the operation's trash before publishing the staged link; original symlinks are
backed up without following them. Original symlink destinations are copied as
real trees into the library, but never edited.

Project copy adoption plans `CopyTree`, then `Claim` (ledger-only, no filesystem
edit), then metadata. It leaves the project copy and `skills-lock.json` unchanged.
Existing identical library trees are reused and not removed by undo. Different
content, including differences hidden by npx hash exclusions, produces
`E_NAME_COLLISION` and no actions for that skill. Project link-mode scopes refuse.

`PlannedAction::authorize_adoption()` captures the complete `before` observation
in `AdoptionAuthorization`, including the exact fingerprint and path. Rebinding
`before` cannot transfer permission to changed content. Apply re-observes targets
under the lock, checks this authorization, and checks `requires` observations
before each effect. Dependent links/claims and metadata refuse if earlier effects
failed or their inputs changed. `approve_modified` cannot substitute for adoption.
Like other engine batches, unexpected failures can leave a partial operation;
only completed effects are journaled, not an all-or-nothing transaction.

Claim, library content, original target and metadata all have guarded inverses
in the same ops row. Undo restores the exact original tree or raw symlink,
restores metadata and ownership, and removes only newly imported library content.
Undo of undo reuses snapshot-bound authorization; a guarded adoption inverse may
restore a previous owner from another scope for shared library metadata.

Lock parsing follows vercel-labs/skills commit
`92a9a34ac3bf88e695e468b90c7bf32f163d32c8`, `src/skill-lock.ts` (global v3,
`~/.agents/.skill-lock.json`) and `src/local-lock.ts` (project v1,
`<project>/skills-lock.json`). Missing locks are optional; invalid/unsupported locks
return `E_CONFIG_PARSE` without writes. GitHub/git entries become Git provenance;
local sources resolve relative to their lock root; other providers retain their
source type, identifier, URL, ref and path as `SkillSource::Npx`. GitHub's global
`skillFolderHash` is a tree SHA, never a commit revision or `base_hash`. Adoption
records the verified actual npx content hash as `base_hash` and retains a v3
`installedAt` timestamp. Existing library provenance is preserved when reusing an
identical item. The planner remains one cohesive operation builder; lock dialect
parsing lives in its private `locks` module.

`skm foreign [--project <dir>]` lists entries and provenance.
`skm adopt <name>... [--project <dir>] [--dry-run]` prints the same exact engine
actions in dry-run mode, without persisting selection, provenance or claims.
Names are single directory names. Refusals exit 3.

## Global skills (T-019)

`global_skills::set_global_skill(&paths, &store, user_home, name, on)` saves
`library.toml` through `library::Library`, using this engine to observe, plan and
apply the global skills links. It returns `ApplyResult`, including
refusal issues and failures. Desired state is retained when a target refuses or
apply fails; saving desired state and applying are not one transaction.

`global_skills::plan_global_skill` takes the same arguments and returns the same
reconciliation plan without saving or applying. On first run its scope id is 0,
for preview only; `set_global_skill` creates the real global scope. Both reconcile
only the toggled skill's target, never other selected skills or their conflicts.
An on request refused by a foreign target still persists its desired state.
Only absent paths and `OurLink` entries are eligible, even if a real copy has a
ledger row. Missing on-skill directories and unsafe names return `E_INVALID_SKILL`.

`Paths::global_skills(user_home)` resolves the injected, existing user home to
`<home>/.agents/skills`. Symlinked descendants are refused by the engine. No agent
registry target or agent-specific skills directory is used. `skm on <skill>` and
`skm off <skill>` call the core API; `--dry-run` prints the plan and issues, and
refusals exit 3. CLI initialization may still create the manager layout, logs and
SQLite database during a dry run, but does not save desired state or apply links.

## Source specs (T-022)

`source::parse(spec, path)` returns `Source::GitHub`, `Git`, `Local`, or
`SkillsSh`. `path: Option<&str>` is the separate `--path` argument, accepted only
with git URLs. Git sources retain their URL, optional ref and relative subpath;
GitHub and skills.sh selections retain owner, repo and skill name (optional for
GitHub). Parsing performs no filesystem or network access. Local paths must be
explicit (`./`, `../`, or absolute) and are retained without home expansion.
GitHub tree URLs use the first segment after `tree/` as the ref. For refs containing
slashes, use a repository URL with `#feature/branch` and a separate `--path`.
Ambiguous specs and conflicting selectors return `E_CONFIG_PARSE` with a specific
correction in the message.
## skills.sh search (T-025)

`search::SearchClient::from_env()` selects `SKILLSMGR_SKILLS_API`, then
`SKILLS_API_URL`, then `https://skills.sh` (empty overrides are ignored).
`SearchClient::new(base_url)` supplies an explicit base URL. `search(query, limit)`
returns `SearchResult` entries with name, source, skill ID and installs;
`SearchResult::install_source()` yields `owner/repo@skill` for the install flow.
Results without an owner/repo source or skill ID are omitted.
Pass 50 for the default UI limit. This is blocking I/O for a worker thread, with
a 3-second total timeout and 1-second DNS/connect timeouts. Failed HTTP requests,
body reads and malformed responses return `E_NETWORK`. Successful results are
cached in memory per client and exact query/limit for 10 minutes, including empty
results. Reuse the client to retain its cache. No downloads or leaderboard calls.
The `live-tests` Cargo feature enables the otherwise excluded live smoke test.
## Projects and sets (T-026)

`projects::register(&store, path)` canonicalizes an existing directory and records
an idempotent SQLite registration with `ProjectMode::Copy` and `auto_sync = true`.
It creates nothing in the repository. `list`, `set_mode`, `set_auto_sync`,
`use_set`, `unuse_set`, `used_sets`, `set_direct_item`, `remove_direct_item`, and
`direct_items` manage machine-local state. They never apply filesystem changes.
Direct selections return `DirectItem { item, agents }`, preserving NULL/inherit
separately from an explicit wildcard agent list.
`unregister` forgets local selections and ownership while leaving files untouched;
callers implementing `--clean` must plan and apply cleanup before unregistering.

Set definitions remain solely in `Library::sets` in `library.toml`.
`Library::{create_set, update_set, delete_set}` mutate that model; persist with
`Library::save`. `ItemSelection::{set_item, remove_item, entries}` handle membership
by `ItemRef` and kind. Plugins are rejected in sets and direct project selections.
Read/list sets through `Library::sets`. Set deletion can leave local project links;
effective calculation treats missing definitions as empty, preserving those links
for a later synced reappearance or explicit `unuse_set`.

`projects::effective` unions selected sets and direct items, deduplicates skills,
and subtracts Global selections per agent. Direct non-skill lists override set
lists, including an empty list. Wildcards expand to enabled registry agents that
support the kind at project scope. Explicit agents are also filtered for enabled
support. `effective_project` loads local selections and enabled agents from SQLite.
A NULL direct agent list inherits set membership when present, otherwise `*`;
an explicit `["*"]` overrides the sets with all enabled supported agents.

`projects::plan_set_change(&paths, &store, &before, &after, name, &registry)`
returns `ProjectPlan { project, plan }` for exactly the projects using that set,
including paused projects so callers can preview pending work. Call it with the
old and new library snapshots, before removing project links. Only skill names
from the old/new set are reconciled, using each project's effective post-edit
selection, mode, and the existing engine's ownership checks. Removed members get
explicit absent targets unless another set or direct selection retains them.
`projects::plan_skills` exposes the same planner for an explicit skill-name scope.
Neither function saves definitions, applies, writes lock files, or edits git
excludes. Copy-mode lock integration, link-mode excludes, and auto-apply scheduling
belong to T-027, T-029, and T-028 respectively. Non-skill selections are retained
for M3 renderers but produce no filesystem actions in M1.
