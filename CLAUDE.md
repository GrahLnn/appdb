## Test File Placement

- All tests must live in dedicated files. Do not add inline `#[cfg(test)] mod tests` blocks inside production source bodies.
- For module-private Rust tests, use a separate sibling test file wired with `#[cfg(test)] mod tests;` or an explicit `#[path = ...]` test module.
- Keep test naming business-agnostic unless the user explicitly requests domain-specific naming.
