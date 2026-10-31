# Engine API (T-017)

`engine::observe(&paths, &store, &targets)` reads exact target fingerprints and
whole-path ledger rows. `engine::plan(scope_id, &desired, &observations)` is pure:
only explicitly supplied targets are reconciled. Request `DesiredContent::Absent`
to turn an item off; omitting it does nothing. The scope must already exist in
SQLite. Paths must be absolute, without `..` or symlink ancestors.

`engine::apply(&paths, &store, &plan, actor, events)` returns completed actions,
refusal issues, failures, and inverse guarded actions in reverse execution order.
It takes a nonblocking fd-lock only during apply. A competing process receives
`E_LOCKED`. Every completed apply, including empty plans and partial batches,
writes an ops row and touches `.changed`; optional events follow lock release.

`Action` contains `Link`, `WriteFile`, `CopyTree`, `Trash`, and `Restore`.
`PlannedAction` pairs the effect with its expected `Observation`, resulting
ownership, and explicit modified-content approval. Apply checks the observation,
ledger, and ownership rule again, even for manually constructed plans. Foreign
targets never become actions. `approve_modified` applies only to ledger-owned
files or trees, not foreign entries.

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

T-020 must introduce explicit, snapshot-bound adoption authorization;
`approve_modified` deliberately cannot authorize foreign targets. Undo acceptance
tests exercise adoption's copy/replace/link effects with a ledger-owned original
directory until that authorization exists.

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
