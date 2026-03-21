# Environment

Environment variables, external dependencies, and setup notes.

**What belongs here:** required tools, workspace setup assumptions, external dependencies, and validation environment notes.
**What does NOT belong here:** service ports or command definitions (use `.factory/services.yaml`).

---

- Project type: Rust workspace with crates `core` and `macros`.
- External services: none required for this mission.
- Validation uses the existing embedded SurrealDB test runtime and temporary directories already used by `core/tests/integration_db.rs`.
- No third-party credentials or network dependencies are required.
- Tooling confirmed available during planning: `cargo`, `cargo fmt`, `cargo clippy`, `trybuild` via Cargo tests.
- Host environment: Windows; prefer PowerShell command syntax when you need shell orchestration.
- Workers must not execute `.sh` files directly on this machine; translate `.factory/init.sh` into Windows-native steps instead.
- The installed GitNexus CLI may not expose the documented `detect_changes` subcommand; when unavailable, use `git diff` plus available `gitnexus impact/context` commands as the approved fallback scope check.
- Known non-blocking note from dry run: `core/tests/integration_db.rs` currently emits a dead-code warning for `NamedFieldTestRelation.created_at`, which may matter for `clippy -D warnings` if left unresolved.
- Nested-reference mission note: compile-pass coverage may need a dedicated trybuild/pass harness because the current repo only has compile-fail wiring in `core/tests/sensitive_compile.rs`.
