# Rust 2024 Upgrade

Mission-specific notes for the workspace edition migration.

**What belongs here:** migration hotspots, validator expectations, and behavior-preservation rules for the Rust 2024 mission.

---

- Upgrade the workspace root to `edition = "2024"` and keep member crates inheriting from the workspace.
- Follow-up metadata target: raise workspace `rust-version` from `1.89.0` to `1.94.0` and keep the validator contract green.
- Public compile surface to preserve:
  - `core/tests/sensitive_compile.rs`
  - `#[derive(Relation)]`
  - `impl_schema!`
- Runtime hotspots already identified during planning:
  - `core/src/auth/mod.rs`
  - `core/src/repository/mod.rs`
  - `core/src/lib.rs`
  - `core/src/crypto.rs`
- Use targeted regressions before broad validators.
- Preserve behavior; do not bundle large let-chains or style cleanups into the edition migration.
