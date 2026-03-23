---
name: rust-library-worker
description: Implement Rust workspace features that span proc-macros, runtime library code, and regression tests.
---

# Rust Library Worker

NOTE: Startup, environment setup, and final cleanup are handled by `worker-base`. This skill defines the work procedure for this mission's Rust library features.

## When to Use This Skill

Use this skill for features that modify `core` and/or `macros` in the `appdb` workspace, especially when the work spans derive macros, repository/runtime behavior, lookup metadata, graph resolution flows, and regression tests.

## Required Skills

- None.

## Work Procedure

1. Read `mission.md`, mission `AGENTS.md`, `.factory/services.yaml`, and relevant `.factory/library/*.md` files before changing code. Rely on worker-base for startup; do not manually invoke `.factory/init.sh` unless the current shell can execute it correctly.
2. If project instructions ask for a GitNexus scope-check command that is unavailable in the current environment, fall back to `git diff` plus any available `gitnexus impact/context` commands; if no GitNexus commands are available at all, use pure `git diff` / `git status` / targeted file inspection and explicitly record that fallback in the handoff.
3. In this mission's mixed Git/Jujutsu environment, prefer `git status` / `git diff` for repository inspection unless the orchestrator explicitly requests Jujutsu commands; avoid `jj status` or similar working-copy commands when a clean Git view is sufficient.
4. The user's current working-tree implementation state is authoritative baseline context for this phase. If core implementation/test files are already dirty when you start, inspect and build on that state unless the feature or orchestrator explicitly instructs a revert.
5. Inspect the exact symbols and tests touched by the feature. Match existing code style and current library patterns; do not invent a second public path if the mission calls for a single main path.
6. Write tests first for the feature's acceptance criteria:
   - integration/runtime coverage in `core/tests/integration_db.rs` or focused test files
   - compile-fail coverage in `core/tests/sensitive_compile.rs` plus `core/tests/ui/**` when macro diagnostics change
   - add a compile-pass harness/fixture when the feature introduces new accepted derive syntax such as `#[foreign]`
    - when a feature adds or changes derive-time validation, keep compile-fail stderr focused on the derive contract itself rather than downstream helper trait bounds
    - after updating compile-fail snapshots, inspect the final stderr text explicitly; trybuild passing is not sufficient if the feature requires one focused derive-time contract diagnostic
   - if a legacy attribute is intentionally removed, prefer compile-fail coverage that asserts Rust's natural attribute error rather than reintroducing custom compatibility diagnostics
   - preserve and extend existing tests instead of duplicating coverage in new files unnecessarily
   - if the requested test filter would match zero tests, add or rename coverage so the filter exercises real tests, or document why a different focused file is the correct seam
7. Run the new or targeted tests and confirm they fail for the expected reason before implementing.
8. Implement the smallest coherent slice that makes the new tests pass while preserving existing behavior for non-sensitive `Store` models.
9. If you introduce a repository-wide storage-conversion trait, preserve automatic identity behavior for plain/manual models and avoid overlapping impl ownership between `Store` and `Sensitive`. Prefer making one side own the conversion impls and the other side provide only encryption/decryption capability.
10. Manually verify any raw-storage assertions required by the feature using the repository's testing/runtime surface. If the feature claims encrypted-at-rest behavior, confirm it through raw DB evidence rather than only in-memory helper assertions.
11. For nested-reference work, prove both halves of the contract: parent rows store only child `RecordId` values, and caller-facing APIs still return hydrated domain children.
12. For this mission's foreign/save regressions, treat failure paths as first-class acceptance criteria: write a failing regression that proves residue exists or could exist, then implement cleanup/atomicity so the failure-path test passes.
13. If a feature claims fresh-DB behavior, use an explicit database reinitialization/reset path in the test itself; do not treat shared helpers like `ensure_db()` as proof of a truly fresh first-save precondition unless the helper itself resets the store in that test.
14. For schemaless follow-up work, model automatic table bootstrap as a Store-wide contract for any persisted type reached in the save graph. Do not implement a foreign-only table-creation exception; `#[foreign]` should only require that the referenced type implements `Store`.
15. For the current cleanup phase, introduce typed error classification before branching on missing-table/not-found/conflict behavior. Repository and foreign-resolution control flow should stop open-coding `contains(...)` checks once the feature is done.
16. When working on explicit-id persistence, converge on one canonical low-level write primitive. If `create_at` survives as create-only semantics, make it a policy wrapper over that primitive rather than a separate builder path.
17. For foreign resolution cleanup, keep explicit-id ensure and lookup-based find-or-create as separate semantic paths. Do not preserve an `exists -> decide -> create` branch for explicit-id foreign children just because it already exists.
18. When changing decode logic, aim for one raw-row normalize/decode pipeline reused by save-return and every read entrypoint. Avoid leaving one-off normalize steps in `get_record`, `list_limit`, or save-return paths.
19. `table_as` work should prove storage identity explicitly. Treat alias and target as different projections over one storage entity, and validate metadata, raw storage shape, and hydrated read surfaces separately.
20. Tests in this phase must be promise-focused. Split schema side effects, unique lookup, foreign hydration, alias storage, and decode compatibility into narrower proofs instead of relying on one large mixed regression.
21. README and rustdoc/API comments are in scope for this phase. If the feature changes the recommended public path or an internal helper's role, update docs/examples in the same feature and verify they match the new API surface.
22. Passing residue-cleanup tests is not sufficient for atomicity claims; inspect the actual commit boundary in `save` / `save_many` and do not mark the feature complete if a later failure would still require post-commit repair.
23. Before changing only the parent transaction wrapper, inspect whether foreign child persistence itself already writes through `T::persist_foreign` / `resolve_foreign_record_id`; if so, move that seam under the authoritative transaction boundary rather than patching only the parent UPSERT path.
24. When a feature claims read-path consistency, compare the same logical row across `save` return, `get`, `get_record`, `list`, and `list_limit`; make the fixture state unambiguous so `list_limit` is proving the intended row.
25. If a hydrate-roundtrip regression fails at `save should succeed`, stop treating it as a read-path-only bug. Trace `Repo::save -> decode_saved_row -> macro-generated decode_stored_row` first, then fix the earliest shared decode seam before resuming `get/get_record/list/list_limit` consistency work.
26. When nested foreign save-return sees string-form record links, do not globally rewrite them into serialized `RecordId` JSON objects or stripped appdb id strings first. Preserve the representation expected by the generated stored structs and `Bridge::hydrate_foreign`; unify the decode seam without inventing a new JSON contract.
27. When tightening record-link parsing, prove both sides: true record-link strings still decode, and ordinary colon-containing payload strings remain plain strings.
28. Record-link parsing fixes must not rely on test-specific table-name prefixes; validate against syntax/shape and include coverage that proves legitimate string-form record ids from more than one table still work.
29. If table-agnostic string-form RecordId compatibility conflicts with plain String payload safety, move the compatibility into a field-type-aware repository/stored-row decode seam rather than broadening the global string rewrite path.
30. When changing hydrate-roundtrip decode logic, run or add at least one regression that exercises a plain `RecordId`-typed model separately from nested foreign models; do not consider nested foreign fixes complete unless that path still passes.
31. If `get_record()` differs from `save`/`get` because it bypasses stored decode, fix that divergence as a stored-decode seam problem first; do not fold caller-facing `Id` stripping into the same recursive rewrite.
32. Before attempting manual nested record-link normalization, inspect the actual `serde_json::to_value(RecordId::new(...))` / `RecordIdKey` representation and build helpers around that exact contract instead of guessing SurrealDB's tagged JSON shape.
33. For hydrate-roundtrip decode work, prefer landing a minimal unit test that fixes the actual `RecordId` serde shape (for example alongside `serde_utils::id`) before changing the shared decode seam.
34. When validating raw-query compatibility, exercise a true string-form record link case (for example ``child:`c1```) rather than only object-form `{ id: ... }` rows.
35. For `#[table_as(...)]` work, prove both target-table raw storage and alias-facing hydrated values; if the alias model itself contains `#[foreign]` fields, validate that nested alias graph across all required read entry points.
36. Run targeted validation during iteration, then run the repo commands from `.factory/services.yaml` before ending the feature. If a validator fails, fix the issue or return to the orchestrator with a precise blocker.
37. Before finalizing a feature, inspect your diff so the change stays within the assigned feature scope; do not leave unrelated sibling-feature edits mixed into the same worker result.
38. If unrelated mission-artifact or validator files are already dirty, review a targeted diff for the touched implementation files and stage only those files for the feature commit. Return to the orchestrator only when the implementation diff itself cannot be safely isolated.
39. If a single shared regression file intentionally accumulates same-milestone test coverage (for example `core/tests/integration_db.rs` in this mission), you may stage the full reviewed file once the remaining hunks are still within that milestone's implementation scope. Do not block solely because earlier same-milestone regressions live in the same file.
40. Before any commit, explicitly inspect `git status --short`, then unstage unrelated pre-existing index entries before `git add` if needed; do not assume the staged set is clean in detached-HEAD mission sessions.
41. Check both `git diff` and `git diff --cached` before commit/final handoff. If pre-existing staged changes outside your feature scope are present and you cannot safely isolate them, return to the orchestrator instead of committing over them.
42. Do not leave background processes running. Avoid watch modes. If you start anything long-lived, stop it before ending the session.
43. In the handoff, be explicit about tests added, commands run, what behavior changed, raw-row evidence collected, failure-path evidence (if any), and any unresolved risks or follow-up work.
