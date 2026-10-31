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

For T-018, deserialize `ops.inverse_json` as `Vec<PlannedAction>` and use it in a
`Plan` with the original `scope_id`. Each inverse expects the exact post-apply
fingerprint and ledger row, so later edits refuse with `E_CONFIG_CHANGED`.
`Restore` copies a hash-verified trash backup, keeping the backup available.
The inverse runner, last-50 selection, and adoption approval belong to T-018 and
T-020, not this ticket. T-020 must introduce an explicit, snapshot-bound adoption
authorization; `approve_modified` deliberately cannot authorize foreign targets.
