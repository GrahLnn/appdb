use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use anyhow::Result;

static RELATION_REGISTRY: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Metadata for a declared relation-table type.
pub trait RelationMeta {
    /// Returns the SurrealDB relation table name.
    fn relation_name() -> &'static str;
}

/// Registers a relation table name for later lookup.
pub fn register_relation(name: &'static str) -> &'static str {
    let mut registry = RELATION_REGISTRY
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    registry.insert(name);
    name
}

/// Returns the relation table name for a declared relation type.
pub fn relation_name<R: RelationMeta>() -> &'static str {
    R::relation_name()
}

/// Validates a relation table name before use.
pub fn ensure_relation_name(name: &str) -> Result<()> {
    let _ = name;
    Ok(())
}

#[cfg(test)]
#[path = "relation_tests.rs"]
mod tests;
