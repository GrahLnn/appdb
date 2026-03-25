# Environment

Environment variables, external dependencies, and setup notes.

**What belongs here:** Required env vars, external dependencies, setup notes.
**What does NOT belong here:** Service ports/commands (use `.factory/services.yaml`).

---

- No new external credentials or services are required for this mission.
- Validation uses the existing Rust workspace and Cargo toolchain in the local repo.
- Windows environment: treat `.sh` files as text instructions only.
- Planned default crypto behavior for this mission:
  - tests use a deterministic test provider/key path
  - non-test default provider uses keyring
  - default service is `appdb` unless overridden globally
  - default account is `master-sensitive` unless overridden globally
- Rust 2024 upgrade mission notes:
  - no new external services or credentials are required
  - workers should inspect the repo for intentional dirty files before editing
  - `.sh` files remain documentation-only in this Windows environment
