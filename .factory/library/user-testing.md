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
  - `cargo check --workspace --all-targets`
  - `cargo test -p appdb --test integration_db -- --test-threads 1`
  - `cargo test -p appdb --test sensitive_compile -- --nocapture`

## Validation Concurrency

- Machine profile observed during planning: 24 logical CPUs, about 127 GB RAM total, about 74 GB free RAM.
- Validator surface cost: moderate. Cargo/trybuild/integration flows are heavier than pure unit tests and some integration tests already serialize embedded DB access.
- Max concurrent validators for this mission surface: **5**.
- Rationale: the machine has ample headroom, the dry run stayed stable, and 5 concurrent validators remains within the 70% headroom rule while staying conservative for compile-heavy Cargo work.

## Gotchas

- Raw storage assertions for encrypted-at-rest behavior should use raw DB query/select evidence, not only in-memory `encrypt/decrypt` helpers.
- `clippy -D warnings` may surface the existing dead-code warning in `core/tests/integration_db.rs`; if it still reproduces after mission changes, treat it as part of this mission's validation work rather than ignoring it.
- For nested-store-reference validation, prove both halves of the contract: raw parent rows store only child `RecordId` values, and caller-facing APIs (`save`, `get`, `list`, `list_limit`) still return hydrated domain children.
- For this mission, also prove `get_record()` matches `get` / `list` / `list_limit` for the same foreign row whenever a contract assertion requires read-path consistency.
- For `list_limit()` assertions, keep the fixture state unambiguous so the row under validation is uniquely identifiable.
- Failure-path validation is in scope: prove that failed `save` / `save_many` calls leave no residue and that a corrected retry on the same identifiers succeeds.
- Raw-query validation must include a string-form record link case such as ``child:`c1``` rather than relying only on object-form `{ id: ... }` rows.
- Alias validation must include a `#[table_as(...)]` model that itself contains `#[foreign]` fields and is then nested again as a foreign child.
- New derive syntax such as `#[foreign]` needs compile-pass and compile-fail evidence; do not rely on runtime tests alone for macro acceptance or diagnostics.
- For enum/manual `Bridge` support, add one compile-pass or runtime roundtrip proving a dispatcher type can persist to a concrete child record id and hydrate back to the original variant, plus compile-fail coverage that illegal foreign+lookup combinations are rejected.
- For `#[derive(Bridge)]`, add compile-pass coverage for single-field tuple enums and compile-fail coverage for unit variants, struct variants, multi-field tuple variants, and payloads that do not implement `Bridge`.
- For recursive foreign containers, add runtime evidence for at least one deep `Option/Vec` shape, proving raw rows preserve the same container structure with `RecordId` leaves and that hydration restores the original nested values.
- Compile-fail coverage should reject wrappers outside the allowed `Option` / `Vec` family, including `Box` and other unsupported containers.
