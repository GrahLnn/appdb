# Sensitive Crypto

Mission-specific facts and conventions for automatic crypto handling.

**What belongs here:** default crypto behavior, override precedence, nested-sensitive rules, metadata expectations.
**What does NOT belong here:** command definitions or service ports (use `.factory/services.yaml`).

---

- Default behavior target:
  - `#[derive(Sensitive)]` automatically uses crypto without caller registration.
  - `#[crypto(...)]` overrides default behavior; it does not enable crypto.
- Planned override precedence for this mission:
  1. field-level `#[crypto(field_account = ...)]`
  2. type-level `#[crypto(service = ..., account = ...)]`
  3. process-wide defaults
  4. built-in fallback defaults
- Runtime auto-ensure should cover both direct runtime-resolver paths and `Store` CRUD paths.
- First-use auto-ensure must be single-flight per model; concurrent cold starts for one model must not race through separate key generation / registration paths and drift to different effective contexts.
- Nested secure shapes in scope: `Child`, `Option<Child>`, `Vec<Child>`.
- Nested sensitive recursion must use an inherited parent-field crypto context for child secure leaves; child models must not attempt top-level self-resolution for nested leaves.
- Treat these as two different seams:
  1. top-level secure field resolver initialization / auto-registration
  2. nested recursive encryption/decryption under an already-resolved parent context
- Current limitation discovered during m3 planning: enum-bearing values inside a secure container need their own supported container/runtime seam; they should not be forced to implement `Sensitive` themselves, and this is distinct from direct `#[secure] Enum` syntax (which remains out of scope).
- Metadata exposure should enumerate secure fields without exposing plaintext values or key material.
- Field-level `#[crypto(...)]` is only valid when the same field is also marked `#[secure]`; stray field crypto attributes on non-secure fields should be treated as compile errors rather than ignored.
- Direct `#[secure] Enum` syntax is out of scope; enum work is limited to roundtrip stability in approved plain/sensitive-contained cases.
- Known but out of scope for the current follow-up: `SensitiveFieldMetadata.secure_fields` has a public semantic inconsistency between model-level constants and per-tag metadata helpers; do not widen current features to fix that unless the requested single-flight or compile-boundary work requires it.
