# Nested Store References

Mission-specific guidance for explicit `#[bindref]` support.

**What belongs here:** supported nested shapes, persistence/hydration rules, child-resolution order, and scope boundaries for the nested-reference mission.
**What does NOT belong here:** command definitions or service ports (use `.factory/services.yaml`).

---

- Explicit opt-in only: nested-reference behavior is enabled only for fields marked `#[bindref]`.
- The bindref persistence seam is `Bridge`: fields persist to `RecordId` through `persist_bindref` and hydrate through `hydrate_bindref`.
- Concrete Store models should work through the default `Bridge` path without extra user code.
- Enum dispatcher types are allowed if they implement `Bridge` manually, and the approved next step is adding `#[derive(Bridge)]` for tuple enums where each variant has exactly one payload field.
- Supported bindref field shapes remain `Child`, `Option<Child>`, and `Vec<Child>`, but `Child` now means any type that implements `Bridge`.
- Default child resolution order:
  1. explicit child record id
  2. existing `UniqueLookupMeta` lookup semantics
  3. create exactly one child row if no existing row matches
- `Bridge` for concrete Store models should preserve the current resolve/create fallback behavior through `resolve_store_ref_record_id` and `Repo::<T>::get_record`.
- `#[derive(Bridge)]` should auto-generate `From<Payload>` impls plus the `Bridge` impl for supported enum shapes.
- Parent rows must store only child `RecordId` values (or arrays thereof) for nested refs.
- Caller-facing Store APIs continue returning hydrated domain objects.
- `Option<Child>::None` should remain empty and must not create child rows.
- `Vec<Child>` should preserve input order after hydration.
- First version should exclude `#[bindref]` fields from automatic lookup candidates and reject `#[unique]` on bindref fields.
- `#[derive(Bridge)]` first version should compile-fail on unit variants, struct variants, multi-field tuple variants, and payloads that do not satisfy `Bridge`.
- First-version boundaries: no lazy-loading API, no `merge`/`patch`/raw query helper support, and no transactional guarantee for parent/child writes.
