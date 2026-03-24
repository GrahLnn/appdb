# Architecture

Architectural decisions and mission-specific patterns.

**What belongs here:** Canonical flows, invariants, narrowed public surfaces, shared design constraints.

---

- `Repo::<T>::delete` string semantics should remain table-local; explicit full-record deletion belongs to `delete_record(RecordId)`.
- Explicit-id foreign persistence must have one authoritative identity; serialized explicit ids and `ResolveRecordId` must not silently diverge.
- `ForeignPersistence` should expose only the operations that remain semantically distinct after the explicit-id ensure split: `exists_record`, `ensure_at`, and `create`.
