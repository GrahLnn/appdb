# Nested Store References

Mission-specific guidance for explicit `#[foreign]` support.

**What belongs here:** supported nested shapes, persistence/hydration rules, child-resolution order, and scope boundaries for the nested-reference mission.
**What does NOT belong here:** command definitions or service ports (use `.factory/services.yaml`).

---

- Explicit opt-in only: nested-reference behavior is enabled only for fields marked `#[foreign]`.
- The foreign persistence seam is `Bridge`: fields persist to `RecordId` through `persist_foreign` and hydrate through `hydrate_foreign`.
- Concrete Store models should work through the default `Bridge` path without extra user code.
- Enum dispatcher types are allowed if they implement `Bridge` manually, and the approved next step is adding `#[derive(Bridge)]` for tuple enums where each variant has exactly one payload field.
- Supported foreign field shapes allow recursive combinations of `Option<_>` and `Vec<_>`, with any leaf type that implements `Bridge`.
- Default child resolution order:
  1. explicit child record id
  2. existing `UniqueLookupMeta` lookup semantics
  3. create exactly one child row if no existing row matches
- `Bridge` for concrete Store models should preserve the current resolve/create fallback behavior through `resolve_foreign_record_id` and `Repo::<T>::get_record`.
- `#[derive(Bridge)]` should auto-generate `From<Payload>` impls plus the `Bridge` impl for supported enum shapes.
- Parent rows must store only child `RecordId` values (or arrays thereof) for nested refs.
- Caller-facing Store APIs continue returning hydrated domain objects.
- `Option<_>::None` should remain empty and must not create child rows at any nesting depth.
- `Vec<_>` should preserve input order after hydration at every nesting level.
- Stored row shape should mirror the caller-facing container shape exactly, with only the leaf values replaced by `RecordId`.
- First version should exclude `#[foreign]` fields from automatic lookup candidates and reject `#[unique]` on foreign fields.
- `#[derive(Bridge)]` first version should compile-fail on unit variants, struct variants, multi-field tuple variants, and payloads that do not satisfy `Bridge`.
- First recursive-container scope now includes only `Option` and `Vec`; reject other wrappers in this task.
- Current scope keeps the no-lazy-loading and no `merge`/`patch` boundaries, but save/save_many now require transactional parent/child semantics for foreign graph persistence.
- Raw query helper writes remain out of scope, but raw-query read compatibility for foreign hydration is in scope.
