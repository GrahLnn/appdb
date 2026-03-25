# User Testing

Validation surface, tools, and concurrency notes.

## Validation Surface

- Primary surface: Rust integration/unit test suite.
- Validation focuses on cargo-based regression tests and compile surface checks for the Rust 2024 edition migration.
- No browser, TUI, or external service surface is involved in this follow-up mission.
- Primary targeted suites for this mission:
  - `core/tests/sensitive_compile.rs`
  - `core/tests/sensitive_roundtrip.rs`
  - focused cases in `core/tests/integration_db.rs`
  - `core/tests/facade_exports.rs`
  - targeted schema / relation compile coverage added for the Rust 2024 migration when needed
- Iteration uses targeted mission tests first; milestone and mission completion should also run the broader repo validators declared in `.factory/services.yaml`.

## Validation Concurrency

- Surface: cargo test / compile checks
- Max concurrent validators: 3
- Rationale: dry run confirmed the cargo-only validation path is stable on this Windows machine, but trybuild and workspace compilation still contend on shared build artifacts, so 3 keeps throughput high without overcommitting compile/cache/IO resources.
- Dry-run resource snapshot: 24 logical CPUs, ~127 GB RAM total, ~70 GB free at planning time.
- Worker iteration guidance: use exact `--test` and test-name filters for `integration_db` before falling back to the broader suite at milestone boundaries.
