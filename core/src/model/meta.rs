use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use surrealdb::types::{RecordId, RecordIdKey, SurrealValue};

static TABLE_REGISTRY: LazyLock<Mutex<HashMap<&'static str, &'static str>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Trait for models that can expose a full SurrealDB record id.
pub trait HasId {
    /// Returns the record id used for graph and direct record operations.
    fn id(&self) -> RecordId;
}

/// Metadata required for repository-style access to a model type.
pub trait ModelMeta:
    Serialize
    + for<'de> Deserialize<'de>
    + SurrealValue
    + std::fmt::Debug
    + 'static
    + Clone
    + Send
    + Sync
{
    /// Returns the storage table name used for this model.
    fn storage_table() -> &'static str {
        Self::table_name()
    }

    /// Returns the table name used for this model.
    fn table_name() -> &'static str;

    /// Builds a record id in the model table.
    fn record_id<T>(id: T) -> RecordId
    where
        RecordIdKey: From<T>,
    {
        RecordId::new(Self::storage_table(), id)
    }
}

/// Metadata used to re-identify one stored record from model field values.
pub trait UniqueLookupMeta {
    /// Field names used for automatic unique lookup.
    fn lookup_fields() -> &'static [&'static str];

    /// Field names that are explicit foreigns and must be excluded from automatic lookup.
    fn foreign_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Narrow marker seam proving a type participates in `#[derive(Store)]`.
#[doc(hidden)]
pub trait StoreModelMarker {}

/// Trait for values that can be resolved to exactly one SurrealDB record id.
#[async_trait::async_trait]
pub trait ResolveRecordId {
    /// Resolves the value to a unique record id.
    async fn resolve_record_id(&self) -> Result<RecordId>;
}

#[async_trait::async_trait]
impl ResolveRecordId for RecordId {
    async fn resolve_record_id(&self) -> Result<RecordId> {
        Ok(self.clone())
    }
}

#[async_trait::async_trait]
impl ResolveRecordId for &RecordId {
    async fn resolve_record_id(&self) -> Result<RecordId> {
        Ok((*self).clone())
    }
}

/// Registers a stable table name for a model type.
pub fn register_table(model: &'static str, table: &'static str) -> &'static str {
    let mut registry = TABLE_REGISTRY.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(existing) = registry.get(model) {
        return existing;
    }
    registry.insert(model, table);
    table
}

/// Converts a Rust type name into the default snake_case table name.
pub fn default_table_name(type_name: &str) -> &'static str {
    let bare = type_name.rsplit("::").next().unwrap_or(type_name);
    let snake = to_snake_case(bare);
    Box::leak(snake.into_boxed_str())
}

fn to_snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    let mut prev_is_lower_or_digit = false;

    for ch in input.chars() {
        if ch.is_ascii_uppercase() {
            if prev_is_lower_or_digit {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_is_lower_or_digit = false;
        } else {
            out.push(ch);
            prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{default_table_name, register_table};

    #[test]
    fn table_name_is_snake_case() {
        assert_eq!(default_table_name("User"), "user");
        assert_eq!(default_table_name("UserProfile"), "user_profile");
        assert_eq!(default_table_name("crate::domain::DbUser"), "db_user");
    }

    #[test]
    fn register_table_is_idempotent_for_model() {
        let first = register_table("ModelA", "alpha");
        let second = register_table("ModelA", "beta");
        assert_eq!(first, "alpha");
        assert_eq!(second, "alpha");
    }
}
