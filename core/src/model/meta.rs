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

    /// Field names backed by `#[foreign]` and resolved to `RecordId` values during lookup.
    fn foreign_fields() -> &'static [&'static str] {
        &[]
    }

    /// Resolves one lookup field into the value that should be bound into the lookup query.
    ///
    /// Plain fields can return `None` to fall back to the model's serialized value, while
    /// `#[foreign]` fields should return their resolved `RecordId` serde shape.
    fn resolve_lookup_field_value(
        &self,
        _field: &str,
    ) -> impl std::future::Future<Output = Result<Option<surrealdb::types::Value>>> {
        async { Ok(None) }
    }
}

/// Metadata describing the default keyset-pagination field for a Store model.
pub trait PaginationMeta {
    /// Field name used for default cursor pagination.
    fn pagination_field() -> Option<&'static str> {
        None
    }
}

/// Query source owned by a View.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSource {
    /// Read from the View's declared Store source table.
    Table,
    /// Read from a typed custom SurrealQL statement.
    Sql,
}

/// Typed bind values for a custom SQL View.
pub trait ViewParams: Send {
    /// Adds this parameter set to a raw SQL statement.
    fn bind_view_params(self, stmt: crate::query::RawSqlStmt) -> Result<crate::query::RawSqlStmt>;
}

impl ViewParams for () {
    fn bind_view_params(self, stmt: crate::query::RawSqlStmt) -> Result<crate::query::RawSqlStmt> {
        Ok(stmt)
    }
}

/// Placeholder source for Views backed by custom SQL instead of one Store table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, surrealdb::types::SurrealValue)]
pub struct NoViewSource;

impl ModelMeta for NoViewSource {
    fn table_name() -> &'static str {
        "__appdb_no_view_source"
    }
}

impl PaginationMeta for NoViewSource {}

/// Metadata and decoder contract for read-only typed projections.
#[async_trait::async_trait]
pub trait ViewMeta:
    Serialize
    + for<'de> Deserialize<'de>
    + SurrealValue
    + std::fmt::Debug
    + 'static
    + Clone
    + Send
    + Sync
{
    /// Store model that owns the source table and write semantics.
    type Source: ModelMeta + PaginationMeta;

    /// Parameter object required by custom SQL Views.
    type Params: ViewParams + Send;

    /// Source kind used by this View.
    fn source_kind() -> ViewSource {
        ViewSource::Table
    }

    /// SurrealQL used when [`Self::source_kind`] returns [`ViewSource::Sql`].
    fn sql() -> Option<&'static str> {
        None
    }

    /// Result-set index decoded for custom SQL Views.
    fn sql_result_index() -> usize {
        0
    }

    /// Stored projection shape decoded from SurrealDB before nested views hydrate.
    type Stored: Clone + serde::de::DeserializeOwned + SurrealValue + Send;

    /// Field names this view is allowed to observe.
    fn view_fields() -> &'static [&'static str];

    /// Declared fields whose values are nested View references.
    fn nested_view_fields() -> &'static [&'static str] {
        &[]
    }

    /// Source table read by this view.
    fn source_table() -> &'static str {
        <Self::Source as ModelMeta>::storage_table()
    }

    /// Source model's pagination field, exposed only for stable ordering.
    fn source_pagination_field() -> Option<&'static str> {
        <Self::Source as PaginationMeta>::pagination_field()
    }

    /// Decodes one projected DB row into the stored projection shape.
    fn decode_stored_view_row(row: serde_json::Value) -> Result<Self::Stored>;

    /// Hydrates nested view fields while preserving the declared projection boundary.
    async fn hydrate_view(stored: Self::Stored) -> Result<Self>;
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
#[path = "meta_tests.rs"]
mod tests;
