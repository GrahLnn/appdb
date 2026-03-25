# Architecture

Architectural decisions and mission-specific patterns.

**What belongs here:** Canonical flows, invariants, narrowed public surfaces, shared design constraints.

---

- `Sensitive` should default to automatic crypto readiness; manual registration is no longer the preferred caller path.
- `#[crypto(...)]` is an override surface, not an enable/disable flag.
- The auto-crypto design should use one stable cached initialization strategy per sensitive model/configuration rather than per-call registration churn.
- Nested sensitive support should come from one recursive abstraction that covers `Child`, `Option<Child>`, and `Vec<Child>` instead of wrapper-by-wrapper special cases.
- `Encrypted*` generation should behave like internal storage plumbing and must not pull unnecessary trait bounds into plaintext domain types.
- Enum work in this mission is about stable roundtrip behavior through `save/get/list/save_many`; direct `#[secure] Enum` syntax remains out of scope.
- For the Rust 2024 edition mission, explicit local bindings are preferred over tail-expression chains in runtime-sensitive code so drop order stays obvious during auth, repository, and crypto flows.
- Treat `cargo fix --edition` output as migration guidance only; keep only changes that remain locally readable and behaviorally justified.
