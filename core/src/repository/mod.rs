use std::marker::PhantomData;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use surrealdb::opt::PatchOp;
use surrealdb::types::{RecordId, RecordIdKey, Table, Value as SurrealDbValue};

use crate::connection::get_db;
use crate::error::DBError;
use crate::model::meta::{HasId, ModelMeta, UniqueLookupMeta};
use crate::query::builder::QueryKind;
use crate::{NestedStoreRefs, StoredModel};

fn struct_field_names<T: Serialize>(data: &T) -> Result<Vec<String>> {
    let value = serde_json::to_value(data)?;
    match value {
        Value::Object(map) => Ok(map.keys().cloned().collect()),
        _ => Ok(vec![]),
    }
}

fn strip_null_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let null_keys = map
                .iter()
                .filter_map(|(key, value)| {
                    if value.is_null() {
                        Some(key.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            for key in null_keys {
                map.remove(&key);
            }

            for nested in map.values_mut() {
                strip_null_fields(nested);
            }
        }
        Value::Array(items) => {
            for nested in items {
                strip_null_fields(nested);
            }
        }
        _ => {}
    }
}

fn extract_record_id_key<T: Serialize>(data: &T) -> Result<RecordIdKey> {
    let value = serde_json::to_value(data)?;
    match value {
        Value::Object(map) => match map.get("id") {
            Some(Value::String(id)) if !id.is_empty() => Ok(RecordIdKey::String(id.clone())),
            Some(Value::Number(id)) => match id.as_i64() {
                Some(id) => Ok(RecordIdKey::Number(id)),
                None => Err(DBError::InvalidModel(format!(
                    "model `{}` has `id` but numeric id is out of i64 range",
                    std::any::type_name::<T>()
                ))
                .into()),
            },
            Some(_) => Err(DBError::InvalidModel(format!(
                "model `{}` has `id` but it is not a non-empty string or i64 number",
                std::any::type_name::<T>()
            ))
            .into()),
            None => Err(DBError::InvalidModel(format!(
                "model `{}` does not contain an `id` string or i64 field",
                std::any::type_name::<T>()
            ))
            .into()),
        },
        _ => Err(DBError::InvalidModel(format!(
            "model `{}` must serialize to an object",
            std::any::type_name::<T>()
        ))
        .into()),
    }
}

fn record_id_key_to_json_value(key: &RecordIdKey) -> Value {
    match key {
        RecordIdKey::String(value) => Value::String(value.clone()),
        RecordIdKey::Number(value) => Value::Number(serde_json::Number::from(*value)),
        _ => unreachable!("extract_record_id_key only returns string or number ids"),
    }
}

fn normalize_store_ref_shapes(value: &mut serde_json::Value) {
    crate::rewrite_store_ref_json_value(value);
    normalize_nested_record_id_json(value);
}

async fn decode_hydrated_row<T>(mut row: serde_json::Value) -> Result<T>
where
    T: NestedStoreRefs,
{
    if let serde_json::Value::Object(map) = &mut row {
        if let Some(id) = map.get_mut("id") {
            *id = match id {
                serde_json::Value::String(_) => {
                    serde_json::Value::String(appdb_id_from_record_string(id))
                }
                _ => id.clone(),
            };
        }

        for (field, value) in map.iter_mut() {
            if field != "id" {
                normalize_store_ref_shapes(value);
            }
        }
    }
    T::hydrate_nested_refs(serde_json::from_value(row)?).await
}

fn prepare_save_parts<T>(table: &str, data: T) -> Result<(RecordId, Value, Value)>
where
    T: Serialize,
{
    let key = extract_record_id_key(&data)?;
    let id = record_id_key_to_json_value(&key);
    let mut content = serde_json::to_value(&data)?;
    if let Value::Object(map) = &mut content {
        map.remove("id");
    }
    strip_null_fields(&mut content);
    let record = RecordId::new(table, key);
    Ok((record, content, id))
}

fn decode_saved_row<T>(row: SurrealDbValue, id: Value) -> Result<T::Stored>
where
    T: NestedStoreRefs,
    T::Stored: serde::de::DeserializeOwned,
{
    let mut row = row.into_json_value();
    if let Value::Object(map) = &mut row {
        map.insert("id".to_owned(), id);
    }

    if T::has_nested_store_refs() {
        if let Value::Object(map) = &mut row {
            for (field, value) in map.iter_mut() {
                if field != "id" {
                    normalize_store_ref_shapes(value);
                }
            }
        }
        Ok(serde_json::from_value(row)?)
    } else {
        Ok(serde_json::from_value(row)?)
    }
}

fn normalize_nested_record_id_json(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        if let Some(id) = map.get_mut("id") {
            *id = serde_json::Value::String(appdb_id_from_record_string(id));
        }

        for nested in map.values_mut() {
            normalize_nested_record_id_json(nested);
        }
    } else if let serde_json::Value::Array(items) = value {
        for nested in items {
            normalize_nested_record_id_json(nested);
        }
    }
}

fn appdb_id_from_record_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text
            .split_once(':')
            .map(|(_, id)| id.trim_matches('`').to_owned())
            .unwrap_or_else(|| text.clone()),
        other => other.to_string(),
    }
}

fn collect_lookup_parts<T>(data: &T) -> Result<Vec<(String, Value)>>
where
    T: UniqueLookupMeta + Serialize,
{
    let value = serde_json::to_value(data)?;
    let Value::Object(map) = value else {
        return Err(DBError::InvalidModel(format!(
            "model `{}` must serialize to an object",
            std::any::type_name::<T>()
        ))
        .into());
    };

    let fields = T::lookup_fields();
    if fields.is_empty() {
        return Err(DBError::InvalidModel(format!(
            "model `{}` has no fields available for automatic unique lookup",
            std::any::type_name::<T>()
        ))
        .into());
    }

    let mut parts = Vec::with_capacity(fields.len());
    for field in fields {
        let value = map.get(*field).cloned().ok_or_else(|| {
            DBError::InvalidModel(format!(
                "model `{}` is missing lookup field `{field}` during automatic unique lookup",
                std::any::type_name::<T>()
            ))
        })?;
        parts.push(((*field).to_owned(), value));
    }

    Ok(parts)
}

async fn stored_rows_to_public_hydrated<T>(rows: Vec<T::Stored>) -> Result<Vec<T>>
where
    T: NestedStoreRefs,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(T::hydrate_nested_refs(row).await?);
    }
    Ok(values)
}

async fn raw_rows_to_public_hydrated<T>(rows: Vec<SurrealDbValue>) -> Result<Vec<T>>
where
    T: NestedStoreRefs,
    T::Stored: serde::de::DeserializeOwned,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        let stored = T::decode_stored_row(row)?;
        values.push(T::hydrate_nested_refs(stored).await?);
    }
    Ok(values)
}

/// Generic repository over a model type registered with [`ModelMeta`].
/// Generic repository helpers for a model type.
pub struct Repo<T>(PhantomData<T>);

impl<T> Repo<T>
where
    T: ModelMeta + StoredModel + NestedStoreRefs,
{
    /// Creates a new row in the model table.
    /// Creates a new row in the model table.
    pub async fn create(data: T) -> Result<T> {
        let db = get_db()?;
        let created: Option<T::Stored> = db
            .create(T::table_name())
            .content(T::persist_nested_refs(data).await?)
            .await?;
        match created {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::EmptyResult("create").into()),
        }
    }

    /// Creates a new row and returns only its record id.
    /// Creates a new row and returns its record id.
    pub async fn create_return_id(data: T) -> Result<RecordId> {
        if !T::supports_create_return_id() {
            return Err(DBError::InvalidModel(format!(
                "model `{}` does not support create_return_id; use create or create_at instead",
                std::any::type_name::<T>()
            ))
            .into());
        }

        let db = get_db()?;
        let stored = T::persist_nested_refs(data).await?;
        let created: Option<RecordId> = db
            .query(QueryKind::create_return_id(T::table_name()))
            .bind(("table", Table::from(T::table_name())))
            .bind(("data", stored))
            .await?
            .check()?
            .take(0)?;
        created.ok_or(DBError::EmptyResult("create_return_id").into())
    }

    /// Creates a new row at the provided record id.
    pub async fn create_at(id: RecordId, data: T) -> Result<T> {
        let db = get_db()?;
        let created: Option<T::Stored> = db
            .create(id)
            .content(T::persist_nested_refs(data).await?)
            .await?;
        match created {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::EmptyResult("create_at").into()),
        }
    }

    /// Upserts a row using [`HasId::id`] as the record id.
    /// Upserts a row using the record id exposed by `HasId`.
    pub async fn upsert(data: T) -> Result<T>
    where
        T: HasId,
    {
        let db = get_db()?;
        let id = data.id();
        let updated: Option<T::Stored> = db
            .upsert(id)
            .content(T::persist_nested_refs(data).await?)
            .await?;
        match updated {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::EmptyResult("upsert").into()),
        }
    }

    /// Upserts a row at the provided record id.
    pub async fn upsert_at(id: RecordId, data: T) -> Result<T> {
        let db = get_db()?;
        let updated: Option<T::Stored> = db
            .upsert(id)
            .content(T::persist_nested_refs(data).await?)
            .await?;
        match updated {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::EmptyResult("upsert_at").into()),
        }
    }

    /// Fetches a row by full record id.
    /// Loads a row by full `RecordId`.
    pub async fn get_record(record: RecordId) -> Result<T> {
        let db = get_db()?;
        let record: Option<T::Stored> = db.select(record).await?;
        match record {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::NotFound.into()),
        }
    }

    /// Replaces the stored content of a row at the provided record id.
    pub async fn update_at(id: RecordId, data: T) -> Result<T> {
        let db = get_db()?;
        let updated: Option<T::Stored> = db
            .update(id)
            .content(T::persist_nested_refs(data).await?)
            .await?;
        match updated {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::NotFound.into()),
        }
    }

    /// Merges a partial JSON object into the row at `id`.
    /// Merges a partial JSON object into an existing row.
    pub async fn merge(id: RecordId, data: Value) -> Result<T> {
        let db = get_db()?;
        let merged: Option<T> = db.update(id).merge(data).await?;
        merged.ok_or(DBError::NotFound.into())
    }

    /// Applies SurrealDB patch operations to the row at `id`.
    /// Applies SurrealDB patch operations to an existing row.
    pub async fn patch(id: RecordId, data: Vec<PatchOp>) -> Result<T> {
        let db = get_db()?;

        if data.is_empty() {
            let record: Option<T> = db.select(id).await?;
            return record.ok_or(DBError::NotFound.into());
        }

        let mut ops = data.into_iter();
        let first_op = ops.next().expect("non-empty patch ops");
        let initial_patch_query = db.update(id).patch(first_op);
        let final_query = ops.fold(initial_patch_query, |query, op| query.patch(op));
        let patched: Option<T> = final_query.await?;
        patched.ok_or(DBError::NotFound.into())
    }

    /// Bulk-inserts rows into the model table.
    /// Inserts many rows using SurrealDB bulk insert.
    pub async fn insert(data: Vec<T>) -> Result<Vec<T>> {
        let db = get_db()?;
        let mut stored = Vec::with_capacity(data.len());
        for item in data {
            stored.push(T::persist_nested_refs(item).await?);
        }
        let created: Vec<T::Stored> = db.insert(T::table_name()).content(stored).await?;
        stored_rows_to_public_hydrated::<T>(created).await
    }

    /// Bulk-inserts rows while ignoring conflicting duplicates.
    /// Inserts many rows while ignoring duplicate-key conflicts.
    pub async fn insert_ignore(data: Vec<T>) -> Result<Vec<T>> {
        let db = get_db()?;
        let chunk_size = 50_000;
        let mut inserted_all = Vec::with_capacity(data.len());

        for chunk in data.chunks(chunk_size) {
            let mut chunk_clone = Vec::with_capacity(chunk.len());
            for item in chunk.iter().cloned() {
                chunk_clone.push(T::persist_nested_refs(item).await?);
            }
            let inserted: Vec<T::Stored> = db
                .query(QueryKind::insert(T::table_name()))
                .bind(("table", Table::from(T::table_name())))
                .bind(("data", chunk_clone))
                .await?
                .check()?
                .take(0)?;
            inserted_all.extend(stored_rows_to_public_hydrated::<T>(inserted).await?);
        }

        Ok(inserted_all)
    }

    /// Bulk-inserts rows and updates existing rows on duplicate keys.
    /// Inserts many rows and updates existing rows on duplicate key.
    pub async fn insert_or_replace(data: Vec<T>) -> Result<Vec<T>> {
        if data.is_empty() {
            return Ok(vec![]);
        }

        let db = get_db()?;
        let chunk_size = 50_000;
        let mut inserted_all = Vec::with_capacity(data.len());
        let keys = struct_field_names(&data[0])?;

        for chunk in data.chunks(chunk_size) {
            let mut chunk_clone = Vec::with_capacity(chunk.len());
            for item in chunk.iter().cloned() {
                chunk_clone.push(T::persist_nested_refs(item).await?);
            }
            let inserted: Vec<T::Stored> = db
                .query(QueryKind::insert_or_replace(T::table_name(), keys.clone()))
                .bind(("table", Table::from(T::table_name())))
                .bind(("data", chunk_clone))
                .await?
                .check()?
                .take(0)?;
            inserted_all.extend(stored_rows_to_public_hydrated::<T>(inserted).await?);
        }

        Ok(inserted_all)
    }

    /// Deletes a row by its table-local `id` value.
    pub async fn delete<K>(id: K) -> Result<()>
    where
        RecordIdKey: From<K>,
        K: Send,
    {
        let record = RecordId::new(T::table_name(), id);
        Self::delete_record(record).await
    }

    /// Deletes one row by full record id.
    /// Deletes a row by full `RecordId`.
    pub async fn delete_record(id: RecordId) -> Result<()> {
        let db = get_db()?;
        db.query(QueryKind::delete_record())
            .bind(("record", id))
            .await?
            .check()?;
        Ok(())
    }

    /// Deletes all rows from the model table.
    /// Deletes every row in the table.
    pub async fn delete_all() -> Result<()> {
        let db = get_db()?;
        let result = db
            .query(QueryKind::delete_table())
            .bind(("table", Table::from(T::table_name())))
            .await?;
        if let Err(err) = result.check() {
            let message = err.to_string();
            if !message.contains("does not exist") {
                return Err(err.into());
            }
        }
        Ok(())
    }

    /// Finds the first record id matching a field equality filter.
    pub async fn find_one_id(k: &str, v: &str) -> Result<RecordId> {
        let db = get_db()?;
        let ids: Vec<RecordId> = db
            .query(QueryKind::select_id_single(T::table_name()))
            .bind(("table", Table::from(T::table_name())))
            .bind(("k", k.to_owned()))
            .bind(("v", v.to_owned()))
            .await?
            .check()?
            .take(0)?;
        let id = ids.into_iter().next();
        id.ok_or(DBError::NotFound.into())
    }

    /// Lists all record ids in the model table.
    /// Lists all record ids in the table.
    pub async fn list_record_ids() -> Result<Vec<RecordId>> {
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::all_id(T::table_name()))
            .bind(("table", Table::from(T::table_name())))
            .await?
            .check()?;
        let ids: Vec<RecordId> = result.take(0)?;
        Ok(ids)
    }

    /// Finds exactly one record id by the model's automatic lookup fields.
    pub async fn find_unique_id_for(data: &T) -> Result<RecordId>
    where
        T: UniqueLookupMeta,
    {
        let db = get_db()?;
        let lookup_parts = collect_lookup_parts(data)?;
        let fields = lookup_parts
            .iter()
            .map(|(field, _)| field.clone())
            .collect::<Vec<_>>();
        let mut query = db
            .query(QueryKind::select_id_by_fields(&fields))
            .bind(("table", Table::from(T::table_name())));

        for (idx, (field, value)) in lookup_parts.into_iter().enumerate() {
            query = query
                .bind((format!("field_{idx}"), field))
                .bind((format!("value_{idx}"), value));
        }

        let mut result = query.await?.check()?;
        let ids: Vec<RecordId> = result.take(0)?;

        match ids.len() {
            1 => Ok(ids.into_iter().next().expect("one id must exist")),
            0 => Err(DBError::NotFound.into()),
            _ => Err(DBError::InvalidModel(
                "automatic unique lookup matched multiple records".to_owned(),
            )
            .into()),
        }
    }
}

impl<T> Repo<T>
where
    T: ModelMeta + StoredModel + NestedStoreRefs,
{
    /// Upserts one model using its `id` field and returns the normalized row.
    /// Saves a model by its `id` field and returns the normalized row.
    pub async fn save(data: T) -> Result<T> {
        let db = get_db()?;
        let stored = T::persist_nested_refs(data).await?;
        let (record, content, id) = prepare_save_parts(T::table_name(), stored)?;
        let row: Option<SurrealDbValue> = db.upsert(record).content(content).await?;
        let row = row.ok_or(DBError::EmptyResult("save"))?;
        let stored = decode_saved_row::<T>(row, id)?;
        T::hydrate_nested_refs(stored).await
    }

    /// Fetches one model by raw id key and normalizes the returned `id`.
    /// Loads a row by its `id` field using the normalized query path.
    pub async fn get<K>(id: K) -> Result<T>
    where
        RecordIdKey: From<K>,
        K: Send,
    {
        let db = get_db()?;
        let key: RecordIdKey = id.into();
        let record = RecordId::new(T::table_name(), key.clone());
        if T::has_nested_store_refs() {
            let stmt = crate::query::RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
                .bind("table", T::table_name())
                .bind("id", key);
            let raw = crate::query::query_bound_return::<serde_json::Value>(stmt)
                .await?
                .ok_or(DBError::NotFound)?;
            return decode_hydrated_row::<T>(raw).await;
        }
        let mut result = db
            .query(QueryKind::select_by_id())
            .bind(("record", record))
            .await?
            .check()?;
        let row: Option<T::Stored> = result.take(0)?;
        match row {
            Some(stored) => Ok(T::hydrate_nested_refs(stored).await?),
            None => Err(DBError::NotFound.into()),
        }
    }

    /// Lists all rows with a normalized `id` field.
    /// Lists all rows with normalized `id` values.
    pub async fn list() -> Result<Vec<T>> {
        if T::has_nested_store_refs() {
            let db = get_db()?;
            let mut result = db
                .query(QueryKind::select_all_with_id())
                .bind(("table", Table::from(T::table_name())))
                .await?
                .check()?;
            let rows: Vec<SurrealDbValue> = result.take(0)?;
            return raw_rows_to_public_hydrated::<T>(rows).await;
        }

        let db = get_db()?;
        let mut result = db
            .query(QueryKind::select_all_with_id())
            .bind(("table", Table::from(T::table_name())))
            .await?
            .check()?;
        let rows: Vec<T::Stored> = result.take(0)?;
        stored_rows_to_public_hydrated::<T>(rows).await
    }

    /// Lists up to `count` rows with a normalized `id` field.
    /// Lists up to `count` rows with normalized `id` values.
    pub async fn list_limit(count: i64) -> Result<Vec<T>> {
        if T::has_nested_store_refs() {
            let db = get_db()?;
            let mut result = db
                .query(QueryKind::select_limit_with_id())
                .bind(("table", Table::from(T::table_name())))
                .bind(("count", count))
                .await?
                .check()?;
            let rows: Vec<SurrealDbValue> = result.take(0)?;
            return raw_rows_to_public_hydrated::<T>(rows).await;
        }

        let db = get_db()?;
        let mut result = db
            .query(QueryKind::select_limit_with_id())
            .bind(("table", Table::from(T::table_name())))
            .bind(("count", count))
            .await?
            .check()?;
        let rows: Vec<T::Stored> = result.take(0)?;
        stored_rows_to_public_hydrated::<T>(rows).await
    }

    /// Batch-upserts models by their `id` field and returns normalized rows.
    /// Saves many rows in chunks and returns normalized results.
    pub async fn save_many(data: Vec<T>) -> Result<Vec<T>> {
        if data.is_empty() {
            return Ok(vec![]);
        }

        let db = get_db()?;
        let mut inserted_all = Vec::with_capacity(data.len());
        let chunk_size = 5_000;

        for chunk in data.chunks(chunk_size) {
            let mut prepared = Vec::with_capacity(chunk.len());
            let mut sql = String::new();

            for (idx, row) in chunk.iter().cloned().enumerate() {
                let (record, content, id) =
                    prepare_save_parts(T::table_name(), T::persist_nested_refs(row).await?)?;
                sql.push_str(&format!(
                    "UPSERT ONLY $record_{idx} CONTENT $data_{idx} RETURN AFTER;"
                ));
                prepared.push((record, content, id));
            }

            let mut query = db.query(sql);
            for (idx, (record, content, _)) in prepared.iter().enumerate() {
                query = query
                    .bind((format!("record_{idx}"), record.clone()))
                    .bind((format!("data_{idx}"), content.clone()));
            }

            let mut result = query.await?.check()?;

            for (idx, (_, _, id)) in prepared.into_iter().enumerate() {
                let row: Option<SurrealDbValue> = result.take(idx)?;
                let row = row.ok_or(DBError::EmptyResult("save_many"))?;
                let stored = decode_saved_row::<T>(row, id)?;
                inserted_all.push(T::hydrate_nested_refs(stored).await?);
            }
        }

        Ok(inserted_all)
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::extract_record_id_key;
    use crate::model::meta::ModelMeta;
    use serde::{Deserialize, Serialize};
    use surrealdb::types::{RecordIdKey, SurrealValue};

    #[derive(Serialize)]
    struct GoodModel {
        id: String,
    }

    #[derive(Serialize)]
    struct MissingId {
        name: String,
    }

    #[derive(Serialize)]
    struct BadIdType {
        id: bool,
    }

    #[derive(Serialize)]
    struct NumberIdType {
        id: i64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
    struct AutoTableModel {
        id: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
    struct CustomTableModel {
        id: String,
    }

    impl ModelMeta for AutoTableModel {
        fn table_name() -> &'static str {
            static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
            TABLE_NAME.get_or_init(|| {
                let table = crate::model::meta::default_table_name(stringify!(AutoTableModel));
                crate::model::meta::register_table(stringify!(AutoTableModel), table)
            })
        }
    }

    impl ModelMeta for CustomTableModel {
        fn table_name() -> &'static str {
            static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
            TABLE_NAME.get_or_init(|| {
                crate::model::meta::register_table(stringify!(CustomTableModel), "custom_users")
            })
        }
    }

    impl crate::StoredModel for AutoTableModel {
        type Stored = Self;

        fn into_stored(self) -> anyhow::Result<Self::Stored> {
            Ok(self)
        }

        fn from_stored(stored: Self::Stored) -> anyhow::Result<Self> {
            Ok(stored)
        }
    }

    impl crate::StoredModel for CustomTableModel {
        type Stored = Self;

        fn into_stored(self) -> anyhow::Result<Self::Stored> {
            Ok(self)
        }

        fn from_stored(stored: Self::Stored) -> anyhow::Result<Self> {
            Ok(stored)
        }
    }

    impl crate::NestedStoreRefs for AutoTableModel {
        async fn persist_nested_refs(value: Self) -> anyhow::Result<Self::Stored> {
            Ok(value)
        }

        async fn hydrate_nested_refs(stored: Self::Stored) -> anyhow::Result<Self> {
            Ok(stored)
        }
    }

    impl crate::NestedStoreRefs for CustomTableModel {
        async fn persist_nested_refs(value: Self) -> anyhow::Result<Self::Stored> {
            Ok(value)
        }

        async fn hydrate_nested_refs(stored: Self::Stored) -> anyhow::Result<Self> {
            Ok(stored)
        }
    }

    impl crate::repository::Crud for AutoTableModel {}
    impl crate::repository::Crud for CustomTableModel {}

    #[test]
    fn extract_id_succeeds_for_valid_model() {
        let model = GoodModel {
            id: "u1".to_owned(),
        };
        assert_eq!(
            extract_record_id_key(&model).expect("expected id"),
            RecordIdKey::String("u1".to_owned())
        );
    }

    #[test]
    fn extract_number_id_succeeds_for_valid_model() {
        let model = NumberIdType { id: 42 };
        assert_eq!(
            extract_record_id_key(&model).expect("expected id"),
            RecordIdKey::Number(42)
        );
    }

    #[test]
    fn extract_id_fails_when_id_missing() {
        let model = MissingId {
            name: "alice".to_owned(),
        };
        let err = extract_record_id_key(&model).expect_err("expected missing id error");
        assert!(err.to_string().contains("does not contain an `id`"));
    }

    #[test]
    fn extract_id_fails_when_id_not_string_or_number() {
        let model = BadIdType { id: true };
        let err = extract_record_id_key(&model).expect_err("expected bad id type error");
        assert!(err
            .to_string()
            .contains("not a non-empty string or i64 number"));
    }

    #[test]
    fn extract_id_fails_when_id_empty() {
        let model = GoodModel { id: String::new() };
        let err = extract_record_id_key(&model).expect_err("expected empty id error");
        assert!(err
            .to_string()
            .contains("not a non-empty string or i64 number"));
    }

    #[test]
    fn default_table_name_from_impl_crud_is_applied() {
        assert_eq!(
            <AutoTableModel as ModelMeta>::table_name(),
            "auto_table_model"
        );
    }

    #[test]
    fn custom_table_name_from_impl_crud_override_is_applied() {
        assert_eq!(
            <CustomTableModel as ModelMeta>::table_name(),
            "custom_users"
        );
    }
}

#[async_trait]
/// Convenience trait that forwards instance-level CRUD calls to [`Repo<Self>`](Repo).
pub trait Crud: ModelMeta + StoredModel + NestedStoreRefs {
    /// Builds a full record id for this model table.
    fn record_id<T>(id: T) -> RecordId
    where
        RecordIdKey: From<T>,
    {
        <Self as ModelMeta>::record_id(id)
    }

    /// Creates a copy of `self` in the database.
    async fn create(&self) -> Result<Self> {
        Repo::<Self>::create(self.clone()).await
    }

    /// Creates a copy of `self` and returns its record id.
    async fn create_return_id(&self) -> Result<RecordId> {
        Repo::<Self>::create_return_id(self.clone()).await
    }

    /// Upserts `self` using its `HasId` implementation.
    async fn upsert(&self) -> Result<Self>
    where
        Self: HasId,
    {
        Repo::<Self>::upsert(self.clone()).await
    }

    /// Loads a row by full `RecordId`.
    async fn get_record(record: RecordId) -> Result<Self> {
        Repo::<Self>::get_record(record).await
    }

    /// Lists all rows with normalized `id` values.
    async fn list() -> Result<Vec<Self>> {
        Repo::<Self>::list().await
    }

    /// Lists up to `count` rows with normalized `id` values.
    async fn list_limit(count: i64) -> Result<Vec<Self>> {
        Repo::<Self>::list_limit(count).await
    }

    /// Replaces the stored content of `self`.
    async fn update(self) -> Result<Self>
    where
        Self: HasId,
    {
        Repo::<Self>::update_at(self.id(), self).await
    }

    /// Replaces the stored content of `self` at the provided record id.
    async fn update_at(self, id: RecordId) -> Result<Self> {
        Repo::<Self>::update_at(id, self).await
    }

    /// Merges a partial JSON object into an existing row.
    async fn merge(id: RecordId, data: Value) -> Result<Self> {
        Repo::<Self>::merge(id, data).await
    }

    /// Applies SurrealDB patch operations to an existing row.
    async fn patch(id: RecordId, data: Vec<PatchOp>) -> Result<Self> {
        Repo::<Self>::patch(id, data).await
    }

    /// Inserts many rows using SurrealDB bulk insert.
    async fn insert(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::insert(data).await
    }

    /// Inserts many rows while ignoring duplicate-key conflicts.
    async fn insert_ignore(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::insert_ignore(data).await
    }

    /// Inserts many rows and updates existing rows on duplicate key.
    async fn insert_or_replace(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::insert_or_replace(data).await
    }

    /// Deletes `self` by its record id.
    async fn delete(self) -> Result<()>
    where
        Self: HasId,
    {
        Repo::<Self>::delete_record(self.id()).await
    }

    /// Deletes a row by full `RecordId`.
    async fn delete_record(id: RecordId) -> Result<()> {
        Repo::<Self>::delete_record(id).await
    }

    /// Deletes every row in the model table.
    async fn delete_all() -> Result<()> {
        Repo::<Self>::delete_all().await
    }

    /// Finds the first record id matching a field equality filter.
    async fn find_one_id(k: &str, v: &str) -> Result<RecordId> {
        Repo::<Self>::find_one_id(k, v).await
    }

    /// Lists all record ids in the model table.
    async fn list_record_ids() -> Result<Vec<RecordId>> {
        Repo::<Self>::list_record_ids().await
    }

    /// Saves `self` using its `id` field and returns the normalized row.
    async fn save(self) -> Result<Self> {
        Repo::<Self>::save(self).await
    }

    /// Loads a row by its `id` field.
    async fn get<T>(id: T) -> Result<Self>
    where
        RecordIdKey: From<T>,
        T: Send,
    {
        Repo::<Self>::get(id).await
    }

    /// Saves many rows in chunks and returns normalized results.
    async fn save_many(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::save_many(data).await
    }
}
