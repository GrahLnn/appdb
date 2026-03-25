---
name: rust-edition-upgrade-worker
description: Implement Rust edition migrations with targeted compile/runtime regressions and full cargo validation.
---

# Rust Edition Upgrade Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Use for features that upgrade this Rust workspace across editions, repair migration fallout, add edition-focused regression coverage, or align validator/documentation surfaces after the edition change.

## Required Skills

None

## Work Procedure

1. Read the assigned feature, `mission.md`, mission `AGENTS.md`, `.factory/services.yaml`, and relevant `.factory/library/*.md` files before editing.
2. On Windows, never execute `.sh` files directly. Read `.factory/init.sh` as documentation only and perform its steps with native commands.
3. Run `git status --short` immediately. If the tree starts dirty, identify whether the feature can proceed safely around those files. Do not discard pre-existing user changes.
4. If GitNexus tooling is available, run impact analysis before editing any function, method, or macro entrypoint you will change. If GitNexus is unavailable, explicitly note the fallback and inspect the affected code/test seams directly.
5. Write or update the narrowest failing regression tests first:
   - trybuild / compile tests for macro or edition-compatibility claims
   - focused runtime tests for drop-order, cleanup, auth, or crypto behavior
   - facade/export tests when verifying public surface integration
6. Run the targeted tests before implementation and capture failing-first proof whenever the path can honestly fail first. If the proof must be an adjusted trybuild stderr or another compile-boundary exception, explain why.
7. Implement the migration fix with explicit, readable code. Treat `cargo fix --edition` as a hint only; prefer manual rewrites that make lifetimes, drop order, and control flow obvious.
8. Re-run targeted regressions until they pass, then run the broader validators from `.factory/services.yaml` that match the feature scope. The final feature in the milestone must run the full validator contract, including `format`.
9. If the feature updates the final project state or validator path, perform the required README adjustment and verify it stays tightly scoped to this mission.
10. Produce a detailed handoff with exact files changed, commands, outcomes, failing-first evidence, and any newly discovered Rust 2024 fallout.

## Example Handoff

```json
{
  "salientSummary": "Upgraded the workspace to edition 2024, added targeted schema/relation compile coverage, and fixed repository tail-expression rewrites that regressed cleanup ordering. Targeted compile/runtime regressions and the full cargo validator contract all passed.",
  "whatWasImplemented": "Changed the workspace edition metadata to 2024, added compile coverage for Rust 2024 schema and Relation surfaces, rewrote runtime-sensitive save/auth paths to use explicit local bindings instead of fragile tail expressions, and aligned the README development section with the final validator commands.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      {
        "command": "cargo test -p appdb --test sensitive_compile -- --nocapture",
        "exitCode": 0,
        "observation": "Compile-pass and compile-fail macro coverage passed under edition 2024."
      },
      {
        "command": "cargo test --workspace -- --test-threads 12",
        "exitCode": 0,
        "observation": "Workspace runtime and integration coverage passed."
      },
      {
        "command": "cargo clippy --workspace --all-targets -- -D warnings",
        "exitCode": 0,
        "observation": "No lint regressions remained after the migration."
      },
      {
        "command": "cargo fmt --all -- --check",
        "exitCode": 0,
        "observation": "Formatting matched repository expectations."
      }
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": [
      {
        "file": "core/tests/sensitive_compile.rs",
        "cases": [
          {
            "name": "relation_derive_rejects_invalid_shape_under_2024",
            "verifies": "Relation derive compile contract remains enforced after the edition upgrade."
          }
        ]
      }
    ]
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- The migration requires scope expansion beyond Rust 2024 compatibility and validator alignment.
- Pre-existing dirty files overlap the same code and make safe progress ambiguous.
- A required edition fix needs a new feature because it touches a different surface than the assigned feature.
- Cargo validation is blocked by environment or toolchain issues you cannot repair locally.
