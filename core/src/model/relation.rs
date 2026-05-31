use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use anyhow::Result;

use crate::error::DBError;

static RELATION_REGISTRY: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Metadata for a declared relation-table type.
pub trait RelationMeta {
    /// Returns the SurrealDB relation table name.
    fn relation_name() -> &'static str;
}

/// Registers a relation table name for later lookup.
pub fn register_relation(name: &'static str) -> &'static str {
    if let Err(err) = ensure_relation_name(name) {
        panic!("{err}");
    }

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
    if is_relation_identifier(name) {
        return Ok(());
    }

    Err(DBError::InvalidIdentifier(format!(
        "relation name `{name}` must be a plain SurrealQL identifier"
    ))
    .into())
}

fn is_relation_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    matches!(first, b'A'..=b'Z' | b'a'..=b'z' | b'_')
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

#[cfg(test)]
#[path = "relation_tests.rs"]
mod tests;
