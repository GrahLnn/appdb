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
2. If project instructions ask for a GitNexus scope-check command that is unavailable in the current environment, fall back to `git diff` plus the available `gitnexus impact/context` commands and explicitly record that fallback in the handoff.
3. In this mission's mixed Git/Jujutsu environment, prefer `git status` / `git diff` for repository inspection unless the orchestrator explicitly requests Jujutsu commands; avoid `jj status` or similar working-copy commands when a clean Git view is sufficient.
4. Inspect the exact symbols and tests touched by the feature. Match existing code style and current library patterns; do not invent a second public path if the mission calls for a single main path.
5. Write tests first for the feature's acceptance criteria:
   - integration/runtime coverage in `core/tests/integration_db.rs` or focused test files
   - compile-fail coverage in `core/tests/sensitive_compile.rs` plus `core/tests/ui/**` when macro diagnostics change
   - add a compile-pass harness/fixture when the feature introduces new accepted derive syntax such as `#[foreign]`
    - when a feature adds or changes derive-time validation, keep compile-fail stderr focused on the derive contract itself rather than downstream helper trait bounds
    - after updating compile-fail snapshots, inspect the final stderr text explicitly; trybuild passing is not sufficient if the feature requires one focused derive-time contract diagnostic
   - if a legacy attribute is intentionally removed, prefer compile-fail coverage that asserts Rust's natural attribute error rather than reintroducing custom compatibility diagnostics
   - preserve and extend existing tests instead of duplicating coverage in new files unnecessarily
   - if the requested test filter would match zero tests, add or rename coverage so the filter exercises real tests, or document why a different focused file is the correct seam
6. Run the new or targeted tests and confirm they fail for the expected reason before implementing.
7. Implement the smallest coherent slice that makes the new tests pass while preserving existing behavior for non-sensitive `Store` models.
8. If you introduce a repository-wide storage-conversion trait, preserve automatic identity behavior for plain/manual models and avoid overlapping impl ownership between `Store` and `Sensitive`. Prefer making one side own the conversion impls and the other side provide only encryption/decryption capability.
9. Manually verify any raw-storage assertions required by the feature using the repository's testing/runtime surface. If the feature claims encrypted-at-rest behavior, confirm it through raw DB evidence rather than only in-memory helper assertions.
10. For nested-reference work, prove both halves of the contract: parent rows store only child `RecordId` values, and caller-facing APIs still return hydrated domain children.
11. For this mission's foreign/save regressions, treat failure paths as first-class acceptance criteria: write a failing regression that proves residue exists or could exist, then implement cleanup/atomicity so the failure-path test passes.
12. When a feature claims read-path consistency, compare the same logical row across `save` return, `get`, `get_record`, `list`, and `list_limit`; make the fixture state unambiguous so `list_limit` is proving the intended row.
13. When validating raw-query compatibility, exercise a true string-form record link case (for example ``child:`c1```) rather than only object-form `{ id: ... }` rows.
14. For `#[table_as(...)]` work, prove both target-table raw storage and alias-facing hydrated values; if the alias model itself contains `#[foreign]` fields, validate that nested alias graph across all required read entry points.
15. Run targeted validation during iteration, then run the repo commands from `.factory/services.yaml` before ending the feature. If a validator fails, fix the issue or return to the orchestrator with a precise blocker.
16. Before finalizing a feature, inspect your diff so the change stays within the assigned feature scope; do not leave unrelated sibling-feature edits mixed into the same worker result.
17. Check both `git diff` and `git diff --cached` before commit/final handoff. If pre-existing staged changes outside your feature scope are present and you cannot safely isolate them, return to the orchestrator instead of committing over them.
18. Do not leave background processes running. Avoid watch modes. If you start anything long-lived, stop it before ending the session.
19. In the handoff, be explicit about tests added, commands run, what behavior changed, raw-row evidence collected, failure-path evidence (if any), and any unresolved risks or follow-up work.
