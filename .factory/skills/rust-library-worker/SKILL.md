---
name: rust-library-worker
description: Implement focused Rust workspace follow-up features with test-first regressions and cargo-based verification.
---

# Rust Library Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Use for Rust workspace features that change library runtime behavior, public protocol surfaces, macros, or integration regressions in this repo.

## Required Skills

None

## Work Procedure

1. Read the assigned feature plus `mission.md`, mission `AGENTS.md`, `.factory/library/*.md`, and `.factory/services.yaml`.
2. Inspect the exact runtime/macro/test seams before editing. For this repo, that often means `macros/src/lib.rs`, `core/src/crypto.rs`, `core/src/lib.rs`, and the relevant files under `core/tests/`.
3. If GitNexus tooling is unavailable, explicitly note the fallback and use direct source inspection plus focused grep/diff review. For this repo, that fallback is acceptable mission compliance when the callable GitNexus integration is absent in the worker session.
4. Add or update focused failing tests first. Prefer the narrowest meaningful surface:
   - compile/UI tests for derive syntax, unsupported shapes, and trait-bound regressions
   - `sensitive_roundtrip` for runtime encryption/decryption behavior
   - focused `integration_db` cases for Store/save/get/list/save_many behavior
5. Run the new or updated tests and confirm they fail for the intended reason before implementation.
6. Implement the minimal coherent runtime/macro change needed to satisfy the feature. Prefer replacing inferior/manual paths over preserving them as the main path.
7. Re-run focused tests until they pass, then run broader validators appropriate to the touched surface from `.factory/services.yaml`.
8. Verify adjacent behavior that could regress:
   - scalar secure fields still work
   - unsupported direct secure-enum syntax stays out of scope
   - batch/save_many behavior remains correct where relevant
9. Produce a handoff with exact commands, observed outcomes, tests added/updated, and any discovered issues or follow-up gaps.

## Example Handoff

```json
{
  "salientSummary": "Tightened Repo::<T>::delete so plain strings remain table-local, added a focused regression for record-id-shaped strings, and verified explicit delete_record(RecordId) still works. Targeted cargo tests and workspace check passed.",
  "whatWasImplemented": "Updated the repository delete path to stop interpreting table-qualified strings as cross-table delete targets, kept delete_record(RecordId) as the explicit full-record path, and added integration coverage for both behaviors.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      {
        "command": "cargo test -p appdb --test integration_db repo_delete_string_id_does_not_cross_table_boundary -- --exact --nocapture",
        "exitCode": 0,
        "observation": "Focused delete regression passed."
      },
      {
        "command": "cargo check --workspace --all-targets",
        "exitCode": 0,
        "observation": "Workspace compiled successfully."
      }
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": [
      {
        "file": "core/tests/integration_db.rs",
        "cases": [
          {
            "name": "repo_delete_string_id_does_not_cross_table_boundary",
            "verifies": "Plain string delete input stays local to the model storage table."
          }
        ]
      }
    ]
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- The feature requires changing mission assertions or milestone structure.
- Current behavior conflicts with the requested narrowing and cannot be resolved without a user-level product decision.
- The validation path is blocked by external environment issues you cannot repair locally.
- The feature reveals a broader design split that should become its own follow-up feature rather than being hidden behind a compatibility shim.
