# Core errors

`skm_core::Error` exposes stable `code()` values, a human-readable message through
`Display`, and a fix hint through `hint()`. Callers must not match message text.

| Code | Meaning | Hint |
| --- | --- | --- |
| `E_FOREIGN_TARGET` | An unmanaged target would be overwritten. | Move the unmanaged target aside or explicitly adopt it. |
| `E_CONFIG_PARSE` | Configuration could not be parsed. | Fix the config syntax before retrying; the file was not changed. |
| `E_CONFIG_CHANGED` | Configuration changed after planning. | Reload the config and create a new plan before applying. |
| `E_GIT_AUTH` | Git authentication or authorization failed. | Check your Git credentials and repository access. |
| `E_GIT_MISSING` | Git is unavailable or older than 2.30. | Install Git >=2.30 with Xcode Command Line Tools (`xcode-select --install`) and make it available on PATH. |
| `E_NETWORK` | A network request failed. | Check your connection and retry. |
| `E_LOCKED` | Another process holds the apply lock. | Wait for the other apply operation to finish and retry. |
| `E_INVALID_SKILL` | Skill metadata or contents are invalid. | Fix the skill metadata or contents and retry. |
| `E_NAME_COLLISION` | Item or target names conflict. | Rename the conflicting item or choose a different target. |
| `E_SECRET_MISSING` | A required secret is unavailable. | Supply the required secret or environment variable. |
| `E_SECRET_IN_PROJECT` | A secret cannot safely be rendered into a project. | Supply the secret through an environment variable; do not write it into the project. |
| `E_PROJECT_EDITED` | Project contents changed after planning. | Review the project edits and create a new plan before applying. |
| `E_ITEM_ACTIVE` | An item is still active and cannot be removed. | Pass --off to turn it off everywhere, or run `skm off` first. |
| `E_IO` | A filesystem operation failed. | Check the path, permissions, and available disk space. |
| `E_LOGGING` | Log file or subscriber initialization failed. | Check the logs directory and initialize logging only once. |
| `E_DATABASE` | Database access, migration, or writer communication failed. | Check the database path and schema version, then reopen the store. |

Foreign adoption uses the existing stable codes: `E_NAME_COLLISION` for different
library content, `E_CONFIG_CHANGED` for stale observations or failed hash checks,
`E_FOREIGN_TARGET` for missing/mismatched adoption permission or unsafe ancestors,
`E_INVALID_SKILL` for non-directory or broken skill sources, and `E_CONFIG_PARSE`
for invalid or unsupported npx locks. No new error variants are introduced.

`skm doctor` reports each check with `status` (`ok`, `warn`, `fail`), `message`,
and `fix` in JSON and human output. It exits 1 for a failed prerequisite, 3 for
warnings only, and 0 when all checks pass. `--bundle <path>` uses `/usr/bin/zip`
to replace the archive with logs and redacted TOML config, including registry
overrides. If config cannot be parsed, its contents and log bodies are omitted
rather than risk exposing secrets. The M3/M5 secrets-scan slot remains inactive
and is not reported as a passing scan.

## Logging

Call `logging::init_logging(&home.join("logs"))` once at application startup and
retain its returned guard until shutdown. It creates the directory and writes
plain-text tracing events to `skillsmgr.log.YYYY-MM-DD`, rotating daily and
retaining at most seven files. Home resolution and CLI/app startup wiring are
owned by the callers. Dropping the guard flushes buffered events.

## Events

`events::channel()` uses `std::sync::mpsc`: core workers can clone the sender and
the GUI bridge owns the receiver. This matches multiple producers and one
consumer without another dependency or an async runtime. The receiver can use
`try_recv()` rather than blocking the GUI thread. Disconnection is returned to
the caller, not swallowed.

`ItemsChanged` and `UpdateFound` carry string item IDs, `SyncState` carries a
state string, and `Error` carries the typed core error. `PlanReady` and `ApplyDone`
are notifications without payloads until the plan/apply tickets define their
data. This skeleton does not define plan or operation models.
