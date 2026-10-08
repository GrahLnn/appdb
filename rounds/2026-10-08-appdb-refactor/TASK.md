# appdb structural refactor

## Original objective

Refactor the Rust and TypeScript implementations of appdb without reverting the SurrealDB 3.3.1 upgrade, remove all direct use of SurrealDB internal APIs, and restore the invariants identified in the architecture audit.

## Acceptance matrix

| Area | Required result | Verification |
| --- | --- | --- |
| Formal model | Lifecycle close, write commit, and free-slot reuse laws pass Bend2 | `wsl bend LAWS.bend`, `wsl bend PROOF.bend --verdict` |
| Rust engine | No direct `surrealdb-core` dependency; workspace compiles on 3.3.1 | `cargo check --workspace --all-targets --locked` |
| Rust writes | Parent and relation writes share one transaction plan; foreign side effects are tracked and cleaned on failure; failed hydration never deletes an existing parent | dedicated Rust integration tests |
| Rust metadata | Table names are cached and do not leak per call | focused metadata test and source inspection |
| TypeScript writes | Finished plans are immutable; model identity is part of planning identity | typecheck, unit tests |
| TypeScript lifecycle | Close is concurrent-safe and idempotent | lifecycle tests |
| Schema | Startup-owned deterministic schema plan; index definition changes are destructive and explicit | schema tests |
| Reads | Rust hydration batches relation lookups; large reads have paged/streaming entry points | repository tests |
| Regression | Existing CRUD, graph, query, crypto, and startup paths remain valid | Rust tests, `pnpm typecheck`, `pnpm test`, `pnpm build` |

## Scope

Only the engine adapter, lifecycle, write planning, metadata, schema ownership, hydration, and read batching paths are in scope. The existing SurrealDB version upgrade remains in place. No compatibility layer for the old internal API is added.

## Current known state

The working copy starts with the SurrealDB 3.3.1 upgrade in `Cargo.toml`, `core/Cargo.toml`, and `Cargo.lock`. Those changes are part of the task baseline and must be preserved.

## Completed implementation

- Replaced the Rust worker and all direct `surrealdb-core` calls with the public `SurrealKv` and `Surreal` SDK surface.
- Made schema application deterministic and startup-owned; generated indexes use destructive `OVERWRITE` DDL only after the Rust sidecar or TypeScript marker fingerprint changes.
- Replaced materialized pagination wrappers with one direct indexed keyset query; generated pagination methods now document the bounded startup path and keep `list()` explicit.
- Cached default table names, batched Rust relation hydration, and removed per-write relation table DDL.
- Added scoped foreign side-effect cleanup and parent cleanup only for records proven absent before the write; existing parents survive post-write decode or hydration failures.
- Made TypeScript close sharing idempotent, made `WritePlan.finish()` immutable, included model identity in planning keys, and moved relation table bootstrap to a client-level once-only path with schema DDL support.
- Added Bend2 lifecycle, commit, and free-slot reuse model laws in `appdb_model.bend`, `LAWS.bend`, and `PROOF.bend`.

## Validation completed

- `cargo test -p appdb --test integration_db`: 126 passed.
- `cargo test --workspace --tests`: passed, including trybuild UI and runtime persistence tests.
- `cargo check --workspace --all-targets --locked`: passed.
- `pnpm run typecheck`, `pnpm test`, and `pnpm run build`: passed; 23 files and 88 tests passed.
- `wsl bash -lc 'cd /mnt/c/Users/admin/appdb && bend PROOF.bend --verdict'`: `ALL PROOFS CHECK`.

## Performance validation

- Added the real-storage child-process benchmark in `core/tests/perf_load.rs`; it uses `init_local_app_db`, keyset pagination, bounded reads, full reads, and CRUD churn across process restarts.
- Release 20k run passed after the schema and pagination changes: baseline reopen 0.584 s, post-churn reopen 0.719 s, first page 22.1 -> 24.8 ms, single get below 0.4 ms, storage 16.25 -> 59.87 MB after three churn rounds.
- Release 100k run passed after the schema and pagination changes: baseline reopen 0.944 s, post-churn reopen 1.119 s, first page 22.3 -> 8.0 ms, single get below 0.4 ms, storage 81.37 -> 154.43 MB after one churn round; four files include the schema fingerprint sidecar.
- Existing relation/graph performance smoke passed: batch relation replacement 2.762 s, single relation replacement 604 ms, graph accessors 1.9–6.7 ms.
- `APPDB_PERF_DIAGNOSE=1` confirmed the pagination index uses `IndexScan`; production pagination now uses that direct bounded query, and unchanged startup skips destructive schema DDL by fingerprint. The benchmark remains a black-box fragmentation proxy rather than a mathematical proof of physical compaction.
- Added `rounds/2026-10-08-appdb-refactor/sqlite_perf.py` and ran the same 20k/100k workload against SQLite 3.50 with `synchronous=FULL`. SQLite reopened in 1–2 ms and kept its file size flat after churn; appdb reopened in 0.584–1.119 s and showed SurrealKV write amplification. Query and churn multipliers are application-path comparisons because SQLite uses prepared SQL while appdb uses its model/repository API.
- Full commands and raw observations are recorded in `rounds/2026-10-08-appdb-refactor/PERF.md`.
