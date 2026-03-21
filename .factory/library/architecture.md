# Architecture

Architectural decisions, discovered patterns, and mission-specific integration guidance.

**What belongs here:** runtime seams, derive interactions, repository behavior, and codebase patterns workers should preserve.
**What does NOT belong here:** service commands/ports (use `.factory/services.yaml`).

---

- `Store` derive lives in `macros/src/lib.rs` and generates `ModelMeta`, `UniqueLookupMeta`, optional `HasId`, `ResolveRecordId`, `Crud`, and inherent CRUD helpers.
- `Sensitive` derive also lives in `macros/src/lib.rs` and currently generates a sibling `Encrypted<Type>` plus explicit `encrypt/decrypt` methods through `appdb::Sensitive`.
- `Repo<T>` in `core/src/repository/mod.rs` currently persists `T` directly. The approved mission introduces the main integration seam here: plaintext model `T` should remain the user-facing type while repository boundaries transparently convert to and from the encrypted storage representation.
- After the first failed CRUD attempt, preserve the newly added grouped `store_sensitive_*` integration tests as the acceptance target; they exposed the real feature gap (secure fields still persisted as plaintext at rest).
- The next CRUD attempt should keep a single owner for persistence conversion logic. Avoid designs where both `Store` and `Sensitive` generate overlapping impls for the same public type.
- Preserve identity behavior for existing manual/plain `Crud` models automatically; do not introduce a new mandatory trait-impl burden for every existing model just to support `Store + Sensitive`.
- Concrete guidance after the second failed CRUD attempt: keep `StoredModel` as the repository seam, but make `#[derive(Store)]` the only owner of `StoredModel` impl generation. Plain Store models should get identity mapping there, while `Store + Sensitive` models should map to `<Self as Sensitive>::Encrypted` through runtime-resolver encrypt/decrypt helpers. `#[derive(Sensitive)]` should not emit `StoredModel` itself.
- Keep a single main path: callers should continue using plaintext `A` with `#[derive(Store, Sensitive)]`; do not add a parallel public persistence API that requires manual `EncryptedA` handling.
- First-version boundary: do not add transparent secure-field handling for `merge`, `patch`, raw query helpers, or `create_return_id` unless the orchestrator adds scope.
- Despite that boundary, `Crud::create_return_id` remains publicly reachable through the generated `Crud` surface for `Store` models. Treat it as an explicit Store + Sensitive API gap until it is either guarded for sensitive models or fully implemented and covered.
- Secure lookup rule: fields marked `#[secure]` must not participate in `#[unique]` or fallback lookup metadata. Mixed models should resolve records only through legal non-secure fields.
- The concrete assertion seam for fallback secure-field exclusion is `UniqueLookupMeta::lookup_fields()`; integration tests can inspect that metadata directly to prove secure fields are absent from automatic lookup candidates.
- Existing runtime and regression anchors:
  - `core/tests/integration_db.rs` for Store/relation behavior
  - `core/tests/sensitive_roundtrip.rs` for encryption behavior
  - `core/tests/sensitive_compile.rs` plus `core/tests/ui/**` for compile-fail coverage
- Nested-store-references mission guidance:
  - `#[store(ref)]` is explicit opt-in only; do not infer nested-reference behavior from child type alone.
  - First-version supported nested shapes are exactly `Child`, `Option<Child>`, and `Vec<Child>`.
  - Parent-facing API values remain domain models; raw parent rows for nested refs must store only child `RecordId` values (or arrays thereof).
  - Child resolution order is: explicit child id first, otherwise existing `UniqueLookupMeta` lookup semantics; if no existing child matches, create exactly one child row.
  - Default read behavior is eager hydration for `get`, `list`, and `list_limit`.
  - First version does not promise transactional parent/child writes and does not expand nested semantics to `merge`, `patch`, or raw query helpers.
