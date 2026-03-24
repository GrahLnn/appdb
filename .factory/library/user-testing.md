# User Testing

Validation surface, tools, and concurrency notes.

## Validation Surface

- Primary surface: Rust integration/unit test suite.
- Validation focuses on cargo-based regression tests and compile surface checks.
- No browser, TUI, or external service surface is involved in this follow-up mission.
- Primary targeted suites for this mission:
  - `core/tests/sensitive_roundtrip.rs`
  - `core/tests/sensitive_compile.rs`
  - focused cases in `core/tests/integration_db.rs`
- Iteration uses targeted mission tests first; milestone and mission completion should also run the broader repo validators declared in `.factory/services.yaml`.

## Validation Concurrency

- Surface: cargo test / compile checks
- Max concurrent validators: 1
- Rationale: the workspace validation path is CPU-heavy and reuses the same build artifacts; serial validator execution is the safest choice on this Windows environment.
- Dry-run resource snapshot: 24 logical CPUs, ~127 GB RAM total, ~68 GB free at planning time.
- Worker iteration guidance: use exact `--test` and test-name filters for `integration_db` before falling back to the broader suite at milestone boundaries.
