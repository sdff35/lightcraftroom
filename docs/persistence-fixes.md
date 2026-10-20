# Persistence fixes: LC-01, LC-02 and LC-03

2026-10-20. These fixes address the three reproduced data-loss risks from the LightCraft audit.
They change the local application only; no cloud backend or server configuration is included.

## LC-01: failed preset saves

A preset mutation previously returned success even when `presets.json` could not be written.
There were no catalog ops to retry, and a successful catalog snapshot at shutdown concealed the
failed preset write.

Preset and profile-list changes now participate in command durability. A failed write returns
`EngineError::NotSaved`, leaves the change in memory, sets the library's save error and retries on
later commands or after the frame-loop backoff. `unsaved()` and `library.info` include one pending
change for the presets file in addition to queued catalog ops. Queries still return their result
while reporting the outstanding save error. A catalog snapshot cannot clear a preset save failure;
the existing quit dialog offers retry, cancel or an explicit quit with unsaved data.

Unreadable settings remain protected against overwriting, but attempts to change those presets
now report a save failure too. No new write format replaces the existing atomic settings-file
writer. Failed changes remain in memory, so retry saving rather than repeating `preset.create`.

## LC-02: duplicate preset IDs

`preset.create` previously used list length and name, so create A, create B, delete A, create B
produced the same ID twice.

Newly created presets use `user.<counter>`. The `nextUserId` counter is stored atomically with the
presets in `presets.json`, including when the last user preset is deleted. Loading an older file
recovers a starting counter from its existing numeric user IDs. Allocation checks existing IDs
and refuses counter exhaustion rather than wrapping.

Loading preserves the first occurrence of an existing ID. Later duplicates, empty IDs and
collisions with built-ins receive distinct IDs without losing their names, groups, favorites or
settings. The repair is reported in the settings warnings and persisted with normal save-error
handling. Reopening the same session reloads the library's preset list rather than treating the
previous in-memory list as more duplicate entries.

These are **local-library identities**, not globally unique cloud IDs. Backend work must add its
own library/UUID mapping and revision contract. Older application versions do not retain the new
counter when rewriting settings; avoid alternating old and fixed writers of the same library.
A preset already discarded by an older version's duplicate filtering can only be recovered from
a backup or original preset export.

## LC-03: catalog locking

Opening a disk library now requires a successfully acquired exclusive OS lock. Failure to open
`catalog.lock`, permission errors and unsupported/failed locking return an actionable error.
They are distinct from another process holding the lock, which keeps its existing owner message.
The library never silently falls back to writable, unlocked operation.

Owner metadata remains informational: failing to write it does not invalidate a successfully
held OS lock. Explicit unlock on drop and OS release after process death remain unchanged.
Do not delete or replace a lock file while an application owns it. On storage without reliable
locks, close all writers and move/copy the library to storage with working exclusive locks.
Browser stores still rely on their host's guard; this change does not implement a cross-tab lock.

## Regression coverage

- `crates/engine/src/tests_persist.rs`: failed preset writes, intact previous saved data,
  outstanding errors after successful catalog writes, snapshot recovery with unsaved presets,
  frame-loop persistence, ID uniqueness and non-reuse after deletion/reopening.
- `crates/engine/src/tests_settings_files.rs`: protected unreadable settings, duplicate-ID
  repair with stable reopening, counter exhaustion and untouched catalog files on lock failure.
- `crates/engine/src/library.rs`: real filesystem preset-write failure, backoff and automatic
  retry without another command.
- `crates/catalog/src/tests_lock.rs` and `lock.rs`: unusable lock path, injected OS locking
  errors, contention, recovery after process death and explicit unlock despite copied descriptors.
- `crates/ui-egui/src/tests_quit_unsaved.rs`: the real headless quit/retry dialog preserves a
  preset whose first save failed.

Before the implementation change, four new engine regressions and the unusable-lock-path test
failed on the original code. Verification commands (use the repository's Rust toolchain):

```sh
cargo test --locked -p lightcraft-engine --lib tests_persist
cargo test --locked -p lightcraft-engine --lib tests_settings_files
cargo test --locked -p lightcraft-engine --lib failed_preset_write_is_retried
cargo test --locked -p lightcraft-catalog --lib lock
cargo xtask ci --summary-md
```

The CI report records platform and missing-fixture limitations in `target/ci/summary.md`.
These fixes unblock backend implementation; production cloud deployment still requires the
separate authentication, resource-limit, synchronization and backup/restore acceptance tests.
