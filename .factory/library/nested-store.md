# Nested Store References

Mission-specific guidance for explicit `#[store(ref)]` support.

**What belongs here:** supported nested shapes, persistence/hydration rules, child-resolution order, and scope boundaries for the nested-reference mission.
**What does NOT belong here:** command definitions or service ports (use `.factory/services.yaml`).

---

- Explicit opt-in only: nested-reference behavior is enabled only for fields marked `#[store(ref)]`.
- First-version supported shapes: `Child`, `Option<Child>`, and `Vec<Child>` where `Child` derives `Store`.
- Default child resolution order:
  1. explicit child record id
  2. existing `UniqueLookupMeta` lookup semantics
  3. create exactly one child row if no existing row matches
- If the macro needs one canonical compile-time obligation for nested-ref children, a narrow exported or `#[doc(hidden)]` Store marker trait is allowed as a diagnostic seam; do not turn it into a second public persistence workflow.
- Parent rows must store only child `RecordId` values (or arrays thereof) for nested refs.
- Caller-facing Store APIs continue returning hydrated domain objects.
- `Option<Child>::None` should remain empty and must not create child rows.
- `Vec<Child>` should preserve input order after hydration.
- First-version boundaries: no lazy-loading API, no `merge`/`patch`/raw query helper support, and no transactional guarantee for parent/child writes.
