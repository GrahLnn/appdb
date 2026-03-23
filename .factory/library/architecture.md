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
  - `#[foreign]` is explicit opt-in only; do not infer nested-reference behavior from child type alone.
  - Introduce a public `Bridge` seam in `core/src/lib.rs`; foreign fields persist by calling `persist_foreign` and hydrate by calling `hydrate_foreign`.
  - Provide the default concrete-model path through a blanket `Bridge` impl for Store children that already satisfy the existing `ModelMeta + ResolveRecordId + Crud + ForeignModel` requirements.
  - Foreign field shapes should support recursive combinations of `Option<_>` and `Vec<_>`, where the leaf types implement `Bridge`.
  - Add `#[derive(Bridge)]` for enum dispatcher types in this scope, but keep the supported shape narrow: each variant must be a single-field tuple variant whose payload already implements `Bridge`.
  - `#[derive(Bridge)]` should auto-generate `From<Payload>` conversions and table-name-based hydrate dispatch; reject unsupported enum shapes with focused derive-time diagnostics.
  - Parent-facing API values remain domain models; raw parent rows for nested refs must store only child `RecordId` values (or arrays thereof).
  - The default concrete-model foreign path keeps the current child resolution order: explicit child id first, otherwise existing `UniqueLookupMeta` lookup semantics; if no existing child matches, create exactly one child row.
  - Current mission raises the bar on write semantics: `save` and `save_many` must be user-visible all-or-nothing across parent rows and auto-persisted foreign children.
  - Post-commit compensating deletes are not an acceptable substitute for that atomicity requirement; the commit boundary itself must not expose residue on an error path.
  - Default read behavior for this mission must be eager hydration across `get`, `get_record`, `list`, and `list_limit`; do not leave a divergent foreign decode path behind.
  - Foreign fields should be excluded from automatic lookup metadata, and `#[unique]` must not be allowed on foreign fields.
  - Recursive foreign support should live in a runtime helper trait adjacent to `Bridge`, not by hard-coding every container depth in the macro. Macros should validate only the allowed wrapper family (`Option`, `Vec`) and generate recursive persist/hydrate calls.
  - Stored field types for foreign containers must preserve the same wrapper shape while recursively replacing only the leaf type with `RecordId`.
  - Raw-query compatibility for this mission includes string-form record links such as ``child:`c1```; decode helpers must normalize them instead of assuming only `{ id: ... }` shapes.
  - `#[table_as(...)]` aliases must continue to resolve through the target table even when the aliased model itself contains `#[foreign]` fields and is later nested again as a foreign child.
  - Keep scope tight: do not expand the new semantics to `merge` or `patch` unless the orchestrator adds scope, but raw-query read compatibility is in scope for foreign hydration.
