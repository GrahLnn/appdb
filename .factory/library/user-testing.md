# User Testing

Validation surfaces, setup expectations, and concurrency guidance for user-testing validators.

**What belongs here:** validation surfaces, dry-run findings, concurrency limits, and runtime testing gotchas.
**What does NOT belong here:** build/test command definitions (use `.factory/services.yaml`).

---

## Validation Surface

- Surface type: Rust library / CLI-style validation rather than browser UI.
- Primary surfaces:
  - unit tests in `core/src/**`
  - integration tests in `core/tests/integration_db.rs`
  - focused runtime tests in `core/tests/sensitive_roundtrip.rs`
  - compile-fail tests in `core/tests/sensitive_compile.rs` with fixtures under `core/tests/ui/**`
- Representative dry run completed successfully during planning:
  - `cargo test --workspace --no-run`
  - `cargo test -p appdb sensitive_record_roundtrip_encrypts_only_secure_fields -- --exact`
  - `cargo test -p appdb --test sensitive_compile -- --nocapture`

## Validation Concurrency

- Machine profile observed during planning: 24 logical CPUs, about 127 GB RAM total, about 74 GB free RAM.
- Validator surface cost: moderate. Cargo/trybuild/integration flows are heavier than pure unit tests and some integration tests already serialize embedded DB access.
- Max concurrent validators for this mission surface: **3**.
- Rationale: the machine has ample headroom, but compile-heavy Cargo flows and embedded DB tests do not benefit linearly from high fan-out; 3 parallel validators stays well within 70% headroom while remaining conservative on test isolation.

## Gotchas

- Raw storage assertions for encrypted-at-rest behavior should use raw DB query/select evidence, not only in-memory `encrypt/decrypt` helpers.
- `clippy -D warnings` may surface the existing dead-code warning in `core/tests/integration_db.rs`; if it still reproduces after mission changes, treat it as part of this mission's validation work rather than ignoring it.
- For nested-store-reference validation, prove both halves of the contract: raw parent rows store only child `RecordId` values, and caller-facing APIs (`save`, `get`, `list`, `list_limit`) still return hydrated domain children.
- New derive syntax such as `#[foreign]` needs compile-pass and compile-fail evidence; do not rely on runtime tests alone for macro acceptance or diagnostics.
- For enum/manual `Bridge` support, add one compile-pass or runtime roundtrip proving a dispatcher type can persist to a concrete child record id and hydrate back to the original variant, plus compile-fail coverage that illegal foreign+lookup combinations are rejected.
- For `#[derive(Bridge)]`, add compile-pass coverage for single-field tuple enums and compile-fail coverage for unit variants, struct variants, multi-field tuple variants, and payloads that do not implement `Bridge`.
- For recursive foreign containers, add runtime evidence for at least one deep `Option/Vec` shape, proving raw rows preserve the same container structure with `RecordId` leaves and that hydration restores the original nested values.
- Compile-fail coverage should reject wrappers outside the allowed `Option` / `Vec` family, including `Box` and other unsupported containers.
