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
2. Inspect the relevant runtime and test seams before editing. If GitNexus tooling is unavailable, note the fallback and use direct source inspection.
3. Write or update focused failing regression tests first in the relevant Rust test surface.
4. Implement the runtime or protocol change only after the regression fails for the intended reason.
5. Run the feature's targeted cargo tests until they pass.
6. Run broader validators from `.factory/services.yaml` that are appropriate before handoff.
7. Verify no inferior parallel path or compatibility shim remains when the feature intends to narrow semantics.
8. Produce a handoff with exact commands, observed outcomes, tests added/updated, and any discovered issues.

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
