use std::marker::PhantomData;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use surrealdb::opt::PatchOp;
use surrealdb::types::{RecordId, RecordIdKey, Table, Value as SurrealDbValue};

use crate::connection::get_db;
use crate::error::{DBError, DBErrorKind, classify_db_error_text};
use crate::model::meta::{HasId, ModelMeta, ResolveRecordId, UniqueLookupMeta};
use crate::query::builder::QueryKind;
use crate::serde_utils::id::parse_record_id_or_plain_string;
use crate::{ForeignModel, RelationWrite, RelationWriteDirection, StoredModel};

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

fn record_id_to_stable_key(record: &RecordId) -> Result<String> {
    let value = serde_json::to_value(record)?;
    Ok(value.to_string())
}

fn normalize_foreign_shapes(value: &mut serde_json::Value) {
    crate::rewrite_foreign_json_value(value);
    crate::decode_stored_record_links(value);
}

fn normalize_declared_foreign_fields<T>(row: &mut serde_json::Value)
where
    T: ForeignModel,
{
    let serde_json::Value::Object(map) = row else {
        return;
    };

    for field in T::foreign_field_names() {
        if let Some(value) = map.get_mut(*field) {
            normalize_foreign_shapes(value);
        }
    }
}

fn decode_error<T>(row: Value, err: serde_json::Error) -> anyhow::Error
where
    T: ModelMeta,
{
    let classified = classify_db_error_text(format!(
        "failed to decode stored `{}` row: {err}; row={row}",
        T::storage_table()
    ));
    debug_assert_eq!(classified.kind, DBErrorKind::Decode);
    classified.into_db_error().into()
}

fn normalize_root_record_id_string(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value
        && let Some(id) = map.get_mut("id")
        && let serde_json::Value::String(text) = id
        && let Ok(record) = parse_record_id_or_plain_string(text, None)
    {
        *id = serde_json::to_value(record).expect("record id should serialize");
    }
}

fn normalize_public_output_ids(value: &mut serde_json::Value) {
    let current_id = value.as_object().and_then(|map| map.get("id")).cloned();

    crate::serde_utils::id::normalize_public_root_id_value(value);

    match current_id {
        Some(serde_json::Value::String(text)) if !text.contains(':') => {
            if let Some(map) = value.as_object_mut() {
                map.insert("id".to_owned(), serde_json::Value::String(text));
            }
        }
        Some(id @ serde_json::Value::Object(_)) => {
            if let Some(map) = value.as_object_mut() {
                map.insert("id".to_owned(), id);
            }
        }
        _ => {}
    }
}

async fn decode_hydrated_row<T>(mut row: serde_json::Value) -> Result<T>
where
    T: ForeignModel + ModelMeta,
{
    let record = record_id_from_row::<T>(&row)?;
    normalize_declared_foreign_fields::<T>(&mut row);
    if T::has_relation_fields() {
        T::inject_relation_values_from_db(record, &mut row).await?;
    }
    normalize_public_output_ids(&mut row);
    T::hydrate_foreign(serde_json::from_value(row)?).await
}

fn record_id_from_row<T>(row: &serde_json::Value) -> Result<RecordId>
where
    T: ModelMeta,
{
    let id = row
        .as_object()
        .and_then(|map| map.get("id"))
        .cloned()
        .ok_or_else(|| DBError::Decode("stored row is missing `id`".to_owned()))?;

    match id {
        serde_json::Value::String(text) => {
            parse_record_id_or_plain_string(&text, Some(T::storage_table())).map_err(|invalid| {
                DBError::Decode(format!("stored row contains invalid id value `{invalid}`")).into()
            })
        }
        serde_json::Value::Object(_) => Ok(serde_json::from_value(id)?),
        other => Err(DBError::Decode(format!(
            "stored row contains unsupported id shape `{other}`"
        ))
        .into()),
    }
}

fn prepare_save_parts<M, T>(table: &str, data: T) -> Result<(RecordId, Value, Value)>
where
    T: Serialize,
    M: ForeignModel,
{
    let key = extract_record_id_key(&data)?;
    let id = record_id_key_to_json_value(&key);
    let record = RecordId::new(table, key);
    Ok((record, prepare_content::<M, _>(data)?, id))
}

fn prepare_content<M, T>(data: T) -> Result<Value>
where
    T: Serialize,
    M: ForeignModel,
{
    let mut content = serde_json::to_value(&data)?;
    if let Value::Object(map) = &mut content {
        map.remove("id");
    }
    M::strip_relation_fields(&mut content);
    strip_null_fields(&mut content);
    Ok(content)
}

async fn sync_relation_writes(writes: &[RelationWrite]) -> Result<()> {
    if writes.is_empty() {
        return Ok(());
    }

    let db = get_db()?;
    for write in writes {
        let delete_sql = match write.direction {
            RelationWriteDirection::Outgoing => "DELETE $rel WHERE in = $record RETURN NONE;",
            RelationWriteDirection::Incoming => "DELETE $rel WHERE out = $record RETURN NONE;",
        };
        let delete_result = db
            .query(delete_sql)
            .bind(("rel", Table::from(write.relation)))
            .bind(("record", write.record.clone()))
            .await?;
        if let Err(err) = delete_result.check() {
            match DBError::from(err) {
                DBError::MissingTable(_) => {}
                other => return Err(other.into()),
            }
        }

        if !write.edges.is_empty() {
            let mut sql = String::from("INSERT RELATION INTO $rel [");
            for idx in 0..write.edges.len() {
                if idx > 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&format!(
                    "{{ in: $in_{idx}, out: $out_{idx}, position: $position_{idx} }}"
                ));
            }
            sql.push_str("] RETURN NONE;");

            let mut query = db.query(sql).bind(("rel", Table::from(write.relation)));
            for (idx, edge) in write.edges.iter().enumerate() {
                let in_record = edge._in.clone().ok_or_else(|| {
                    DBError::InvalidModel(
                        "relation edge write is missing its source record id".to_owned(),
                    )
                })?;
                query = query
                    .bind((format!("in_{idx}"), in_record))
                    .bind((format!("out_{idx}"), edge.out.clone()))
                    .bind((format!("position_{idx}"), edge.position));
            }
            query.await?.check()?;
        }
    }

    Ok(())
}

async fn persist_explicit_id_primitive<T>(record: RecordId, data: T, create_only: bool) -> Result<T>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    let db = get_db()?;
    let original = data.clone();
    let stored_input = T::persist_foreign(data).await?;
    let content = prepare_content::<T, _>(stored_input)?;
    let statement = if create_only {
        "BEGIN TRANSACTION; CREATE ONLY $record CONTENT $data RETURN AFTER; COMMIT TRANSACTION;"
    } else {
        "BEGIN TRANSACTION; UPSERT ONLY $record CONTENT $data RETURN AFTER; COMMIT TRANSACTION;"
    };

    let result = db
        .query(statement)
        .bind(("record", record.clone()))
        .bind(("data", content))
        .await;
    let mut result = match result {
        Ok(result) => result,
        Err(err) => {
            let typed = DBError::from(err);
            return if create_only && matches!(typed, DBError::EmptyResult(_)) {
                Err(DBError::Conflict("record already exists".to_owned()).into())
            } else {
                Err(typed.into())
            };
        }
    };
    result = match result.check() {
        Ok(result) => result,
        Err(err) => {
            let typed = DBError::from(err);
            return if create_only && matches!(typed, DBError::EmptyResult(_)) {
                Err(DBError::Conflict("record already exists".to_owned()).into())
            } else {
                Err(typed.into())
            };
        }
    };

    let row: Option<SurrealDbValue> = result.take(1)?;
    let row = row.ok_or_else(|| {
        if create_only {
            DBError::Conflict("record already exists".to_owned())
        } else {
            DBError::EmptyResult("persist_explicit_id_primitive")
        }
    })?;

    let relation_writes = original.prepare_relation_writes(record.clone()).await?;
    sync_relation_writes(&relation_writes).await?;
    let stored =
        decode_saved_row_from_model::<T>(row, serde_json::to_value(record.clone())?, &original)?;
    let mut value = serde_json::to_value(T::hydrate_foreign(stored).await?)?;
    normalize_public_output_ids(&mut value);
    Ok(serde_json::from_value(value)?)
}

fn decode_saved_row_from_model<T>(row: SurrealDbValue, id: Value, model: &T) -> Result<T::Stored>
where
    T: ForeignModel + ModelMeta,
    T::Stored: serde::de::DeserializeOwned,
{
    let mut row = row.into_json_value();
    if let Value::Object(map) = &mut row {
        map.insert("id".to_owned(), id);
    }
    normalize_root_record_id_string(&mut row);
    normalize_declared_foreign_fields::<T>(&mut row);
    model.inject_relation_values_from_model(&mut row)?;
    serde_json::from_value(row.clone()).map_err(|err| decode_error::<T>(row, err))
}

fn decode_stored_row_value<T>(mut row: Value, id: Option<Value>) -> Result<T::Stored>
where
    T: ForeignModel + ModelMeta,
    T::Stored: serde::de::DeserializeOwned,
{
    if let Value::Object(map) = &mut row
        && let Some(id) = id
    {
        map.insert("id".to_owned(), id);
    }

    normalize_root_record_id_string(&mut row);
    normalize_declared_foreign_fields::<T>(&mut row);

    serde_json::from_value(row.clone()).map_err(|err| decode_error::<T>(row, err))
}

pub(crate) async fn record_exists(record: RecordId) -> Result<bool> {
    let db = get_db()?;
    let selected: std::result::Result<Option<SurrealDbValue>, surrealdb::Error> =
        db.select(record).await;
    match selected {
        Ok(existing) => Ok(existing.is_some()),
        Err(err) => match crate::error::classify_surreal_error(err) {
            crate::error::DBError::MissingTable(_) => Ok(false),
            other => Err(other.into()),
        },
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
    T: ForeignModel,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(T::hydrate_foreign(row).await?);
    }
    Ok(values)
}

async fn decode_stored_row_from_db<T>(mut row: Value) -> Result<T::Stored>
where
    T: ForeignModel + ModelMeta,
    T::Stored: serde::de::DeserializeOwned,
{
    let record = record_id_from_row::<T>(&row)?;
    normalize_root_record_id_string(&mut row);
    normalize_declared_foreign_fields::<T>(&mut row);
    if T::has_relation_fields() {
        T::inject_relation_values_from_db(record, &mut row).await?;
    }
    serde_json::from_value(row.clone()).map_err(|err| decode_error::<T>(row, err))
}

pub(crate) async fn raw_rows_to_public_hydrated<T>(rows: Vec<SurrealDbValue>) -> Result<Vec<T>>
where
    T: ForeignModel + ModelMeta,
    T::Stored: serde::de::DeserializeOwned,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        let stored = decode_stored_row_from_db::<T>(row.into_json_value()).await?;
        values.push(T::hydrate_foreign(stored).await?);
    }
    Ok(values)
}

/// Internal repository building blocks for a model type.
///
/// This type remains public for advanced integration seams and mission-internal
/// tests, but application code should prefer the narrower model-facing CRUD
/// methods generated by `#[derive(Store)]` and the [`Crud`] trait wrappers.
pub struct Repo<T>(PhantomData<T>);

impl<T> Repo<T>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    /// Creates a new row in the model table.
    /// Creates a new row in the model table.
    pub async fn create(data: T) -> Result<T> {
        if T::has_relation_fields() {
            let db = get_db()?;
            let original = data.clone();
            let stored_input = T::persist_foreign(data).await?;
            let content = prepare_content::<T, _>(stored_input)?;
            let mut result = db
                .query("BEGIN TRANSACTION; CREATE $table CONTENT $data RETURN AFTER; COMMIT TRANSACTION;")
                .bind(("table", Table::from(T::storage_table())))
                .bind(("data", content))
                .await?
                .check()?;
            let row: Option<SurrealDbValue> = result.take(1)?;
            let row = row.ok_or(DBError::EmptyResult("create"))?;
            let row_json = row.into_json_value();
            let record = record_id_from_row::<T>(&row_json)?;
            let relation_writes = original.prepare_relation_writes(record).await?;
            sync_relation_writes(&relation_writes).await?;
            let stored = decode_stored_row_from_db::<T>(row_json).await?;
            return Ok(T::hydrate_foreign(stored).await?);
        }

        let db = get_db()?;
        let created: Option<T::Stored> = db
            .create(T::storage_table())
            .content(T::persist_foreign(data).await?)
            .await?;
        match created {
            Some(stored) => Ok(T::hydrate_foreign(stored).await?),
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

        if T::has_relation_fields() {
            return Err(DBError::InvalidModel(
                "create_return_id is not supported for models with #[relate(...)] fields"
                    .to_owned(),
            )
            .into());
        }

        let db = get_db()?;
        let stored = T::persist_foreign(data).await?;
        let created: Option<RecordId> = db
            .query(QueryKind::create_return_id(T::storage_table()))
            .bind(("table", Table::from(T::storage_table())))
            .bind(("data", stored))
            .await?
            .check()?
            .take(0)?;
        created.ok_or(DBError::EmptyResult("create_return_id").into())
    }

    /// Creates a new row at the provided record id.
    pub async fn create_at(id: RecordId, data: T) -> Result<T> {
        persist_explicit_id_primitive::<T>(id, data, true).await
    }

    /// Upserts a row using [`HasId::id`] as the record id.
    /// Upserts a row using the record id exposed by `HasId`.
    pub async fn upsert(data: T) -> Result<T>
    where
        T: HasId,
    {
        let id = data.id();
        Self::upsert_at(id, data).await
    }

    /// Upserts a row at the provided record id.
    pub async fn upsert_at(id: RecordId, data: T) -> Result<T> {
        persist_explicit_id_primitive::<T>(id, data, false).await
    }

    /// Fetches a row by full record id.
    /// Loads a row by full `RecordId`.
    pub async fn get_record(record: RecordId) -> Result<T> {
        let db = get_db()?;
        let requested = record.clone();
        let record: Option<SurrealDbValue> = db.select(record).await?;
        match record {
            Some(stored) => {
                let stored = if T::has_relation_fields() {
                    let mut row = stored.into_json_value();
                    if let Value::Object(map) = &mut row {
                        map.insert("id".to_owned(), serde_json::to_value(requested.clone())?);
                    }
                    decode_stored_row_from_db::<T>(row).await?
                } else {
                    decode_stored_row_value::<T>(
                        stored.into_json_value(),
                        Some(serde_json::to_value(requested)?),
                    )?
                };
                let mut value = serde_json::to_value(T::hydrate_foreign(stored).await?)?;
                normalize_public_output_ids(&mut value);
                Ok(serde_json::from_value(value)?)
            }
            None => Err(DBError::NotFound.into()),
        }
    }

    pub async fn exists_record(record: RecordId) -> Result<bool> {
        record_exists(record).await
    }

    /// Replaces the stored content of a row at the provided record id.
    pub async fn update_at(id: RecordId, data: T) -> Result<T> {
        if T::has_relation_fields() {
            let db = get_db()?;
            let original = data.clone();
            let stored_input = T::persist_foreign(data).await?;
            let content = prepare_content::<T, _>(stored_input)?;
            let mut result = db
                .query(
                    "BEGIN TRANSACTION; UPDATE $record CONTENT $data RETURN AFTER; COMMIT TRANSACTION;",
                )
                .bind(("record", id.clone()))
                .bind(("data", content))
                .await?
                .check()?;
            let row: Option<SurrealDbValue> = result.take(1)?;
            let row = row.ok_or(DBError::NotFound)?;
            let relation_writes = original.prepare_relation_writes(id.clone()).await?;
            sync_relation_writes(&relation_writes).await?;
            let stored =
                decode_saved_row_from_model::<T>(row, serde_json::to_value(id)?, &original)?;
            let mut value = serde_json::to_value(T::hydrate_foreign(stored).await?)?;
            normalize_public_output_ids(&mut value);
            return Ok(serde_json::from_value(value)?);
        }

        let db = get_db()?;
        let updated: Option<T::Stored> = db
            .update(id)
            .content(T::persist_foreign(data).await?)
            .await?;
        match updated {
            Some(stored) => Ok(T::hydrate_foreign(stored).await?),
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
        if T::has_relation_fields() {
            return Err(DBError::InvalidModel(
                "insert is not supported for models with #[relate(...)] fields; use save_many"
                    .to_owned(),
            )
            .into());
        }

        let db = get_db()?;
        let mut stored = Vec::with_capacity(data.len());
        for item in data {
            stored.push(T::persist_foreign(item).await?);
        }
        let created: Vec<T::Stored> = db.insert(T::storage_table()).content(stored).await?;
        stored_rows_to_public_hydrated::<T>(created).await
    }

    /// Bulk-inserts rows while ignoring conflicting duplicates.
    /// Inserts many rows while ignoring duplicate-key conflicts.
    pub async fn insert_ignore(data: Vec<T>) -> Result<Vec<T>> {
        if T::has_relation_fields() {
            return Err(DBError::InvalidModel(
                "insert_ignore is not supported for models with #[relate(...)] fields; use save_many"
                    .to_owned(),
            )
            .into());
        }

        let db = get_db()?;
        let chunk_size = 50_000;
        let mut inserted_all = Vec::with_capacity(data.len());

        for chunk in data.chunks(chunk_size) {
            let mut chunk_clone = Vec::with_capacity(chunk.len());
            for item in chunk.iter().cloned() {
                chunk_clone.push(T::persist_foreign(item).await?);
            }
            let inserted: Vec<T::Stored> = db
                .query(QueryKind::insert(T::storage_table()))
                .bind(("table", Table::from(T::storage_table())))
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
        if T::has_relation_fields() {
            return Err(DBError::InvalidModel(
                "insert_or_replace is not supported for models with #[relate(...)] fields; use save_many"
                    .to_owned(),
            )
            .into());
        }

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
                chunk_clone.push(T::persist_foreign(item).await?);
            }
            let inserted: Vec<T::Stored> = db
                .query(QueryKind::insert_or_replace(
                    T::storage_table(),
                    keys.clone(),
                ))
                .bind(("table", Table::from(T::storage_table())))
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
        let key: RecordIdKey = id.into();
        let record = match key {
            RecordIdKey::String(text) => RecordId::new(T::storage_table(), text),
            other => RecordId::new(T::storage_table(), other),
        };
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
            .bind(("table", Table::from(T::storage_table())))
            .await?;
        if let Err(err) = result.check() {
            match DBError::from(err) {
                DBError::MissingTable(_) => {}
                other => return Err(other.into()),
            }
        }
        Ok(())
    }

    /// Finds the first record id matching a field equality filter.
    pub async fn find_one_id(k: &str, v: &str) -> Result<RecordId> {
        let db = get_db()?;
        let ids: Vec<RecordId> = db
            .query(QueryKind::select_id_single(T::storage_table()))
            .bind(("table", Table::from(T::storage_table())))
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
            .query(QueryKind::all_id(T::storage_table()))
            .bind(("table", Table::from(T::storage_table())))
            .await?
            .check()?;
        let ids: Vec<RecordId> = result.take(0)?;
        Ok(ids)
    }

    /// Returns whether the model table currently contains at least one row.
    pub async fn exists() -> Result<bool> {
        let db = get_db()?;
        let mut result = match db
            .query(QueryKind::table_has_rows(T::storage_table()))
            .bind(("table", Table::from(T::storage_table())))
            .await
        {
            Ok(result) => match result.check() {
                Ok(result) => result,
                Err(err) => match DBError::from(err) {
                    DBError::MissingTable(_) => return Ok(false),
                    other => return Err(other.into()),
                },
            },
            Err(err) => match DBError::from(err) {
                DBError::MissingTable(_) => return Ok(false),
                other => return Err(other.into()),
            },
        };

        let exists: Option<bool> = result.take(0)?;
        match exists {
            Some(exists) => Ok(exists),
            None => Ok(false),
        }
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
            .bind(("table", Table::from(T::storage_table())));

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
    T: ModelMeta + StoredModel + ForeignModel,
{
    /// Upserts one model using its `id` field and returns the normalized row.
    /// Saves a model by its `id` field and returns the normalized row.
    pub async fn save(data: T) -> Result<T> {
        if !T::has_foreign_fields() && extract_record_id_key(&data).is_ok() {
            let record = RecordId::new(T::storage_table(), extract_record_id_key(&data)?);
            return persist_explicit_id_primitive::<T>(record, data, false).await;
        }

        let db = get_db()?;
        let original = data.clone();
        let (stored, created_foreign_records) =
            crate::run_with_foreign_cleanup_scope(|| async { T::persist_foreign(data).await })
                .await?;
        let (record, content, id) = prepare_save_parts::<T, _>(T::storage_table(), stored)?;
        let mut result = db
            .query("BEGIN TRANSACTION; UPSERT ONLY $record CONTENT $data RETURN AFTER; COMMIT TRANSACTION;")
            .bind(("record", record.clone()))
            .bind(("data", content.clone()))
            .await?
            .check()?;
        let row: Option<SurrealDbValue> = result.take(1)?;
        let row = row.ok_or(DBError::EmptyResult("save"))?;
        let relation_writes = original.prepare_relation_writes(record.clone()).await?;
        sync_relation_writes(&relation_writes).await?;
        let stored = decode_saved_row_from_model::<T>(row, id, &original)?;
        match T::hydrate_foreign(stored).await {
            Ok(value) => Ok(value),
            Err(err) => {
                let _: Option<SurrealDbValue> = db.delete(record).await?;
                for foreign_record in created_foreign_records.into_iter().rev() {
                    let _: Option<SurrealDbValue> = db.delete(foreign_record).await?;
                }
                Err(err)
            }
        }
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
        let record = RecordId::new(T::storage_table(), key.clone());
        if T::has_foreign_fields() || T::has_relation_fields() {
            let stmt = crate::query::RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
                .bind("table", T::storage_table())
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
            Some(stored) => {
                let mut value = serde_json::to_value(T::hydrate_foreign(stored).await?)?;
                normalize_public_output_ids(&mut value);
                Ok(serde_json::from_value(value)?)
            }
            None => Err(DBError::NotFound.into()),
        }
    }

    /// Lists all rows with a normalized `id` field.
    /// Lists all rows with normalized `id` values.
    pub async fn list() -> Result<Vec<T>> {
        if T::has_foreign_fields() || T::has_relation_fields() {
            let db = get_db()?;
            let mut result = db
                .query(QueryKind::select_all_with_id())
                .bind(("table", Table::from(T::storage_table())))
                .await?
                .check()?;
            let rows: Vec<SurrealDbValue> = result.take(0)?;
            return raw_rows_to_public_hydrated::<T>(rows).await;
        }

        let db = get_db()?;
        let mut result = db
            .query(QueryKind::select_all_with_id())
            .bind(("table", Table::from(T::storage_table())))
            .await?
            .check()?;
        let rows: Vec<T::Stored> = result.take(0)?;
        stored_rows_to_public_hydrated::<T>(rows).await
    }

    /// Lists up to `count` rows with a normalized `id` field.
    /// Lists up to `count` rows with normalized `id` values.
    pub async fn list_limit(count: i64) -> Result<Vec<T>> {
        if T::has_foreign_fields() || T::has_relation_fields() {
            let db = get_db()?;
            let mut result = db
                .query(QueryKind::select_limit_with_id())
                .bind(("table", Table::from(T::storage_table())))
                .bind(("count", count))
                .await?
                .check()?;
            let rows: Vec<SurrealDbValue> = result.take(0)?;
            return raw_rows_to_public_hydrated::<T>(rows).await;
        }

        let db = get_db()?;
        let mut result = db
            .query(QueryKind::select_limit_with_id())
            .bind(("table", Table::from(T::storage_table())))
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
            let mut originals = Vec::with_capacity(chunk.len());
            let mut relation_writes = Vec::new();
            let mut sql = String::from("BEGIN TRANSACTION; ");
            let mut created_foreign_records = Vec::new();
            let mut seen_records = std::collections::HashSet::<String>::with_capacity(chunk.len());

            for (idx, row) in chunk.iter().cloned().enumerate() {
                let original = row.clone();
                let ((record, content, id), row_foreign_records) =
                    crate::run_with_foreign_cleanup_scope(|| async {
                        let stored_row = T::persist_foreign(row).await?;
                        let (record, content, id) =
                            prepare_save_parts::<T, _>(T::storage_table(), stored_row)?;
                        Ok::<_, anyhow::Error>((record, content, id))
                    })
                    .await?;
                let record_key = record_id_to_stable_key(&record)?;
                if !seen_records.insert(record_key) {
                    return Err(DBError::Conflict(format!(
                        "save_many received duplicate record id in one batch: {record:?}"
                    ))
                    .into());
                }
                created_foreign_records.extend(row_foreign_records);
                relation_writes.extend(original.prepare_relation_writes(record.clone()).await?);
                sql.push_str(&format!(
                    "UPSERT ONLY $record_{idx} CONTENT $data_{idx} RETURN AFTER;"
                ));
                originals.push(original);
                prepared.push((record, content, id));
            }
            sql.push_str("COMMIT TRANSACTION;");

            let mut query = db.query(sql);
            for (idx, (record, content, _)) in prepared.iter().enumerate() {
                query = query
                    .bind((format!("record_{idx}"), record.clone()))
                    .bind((format!("data_{idx}"), content.clone()));
            }

            let mut result = query.await?.check()?;
            sync_relation_writes(&relation_writes).await?;

            for (idx, (_, _, id)) in prepared.clone().into_iter().enumerate() {
                let row: Option<SurrealDbValue> = result.take(idx + 1)?;
                let row = row.ok_or(DBError::EmptyResult("save_many"))?;
                let stored = decode_saved_row_from_model::<T>(row, id, &originals[idx])?;
                match T::hydrate_foreign(stored).await {
                    Ok(value) => inserted_all.push(value),
                    Err(err) => {
                        for (record, _, _) in prepared.iter() {
                            let _: Option<SurrealDbValue> = db.delete(record.clone()).await?;
                        }
                        for foreign_record in created_foreign_records.into_iter().rev() {
                            let _: Option<SurrealDbValue> = db.delete(foreign_record).await?;
                        }
                        return Err(err);
                    }
                }
            }
        }

        Ok(inserted_all)
    }
}

#[async_trait]
/// Recommended model-facing CRUD surface.
///
/// `#[derive(Store)]` forwards its inherent methods through this trait so caller
/// code can stay on the domain model type instead of reaching for [`Repo`]
/// directly. Treat [`Repo`] as an internal composition layer unless you are
/// extending appdb itself or wiring a custom runtime seam.
pub trait Crud: ModelMeta + StoredModel + ForeignModel {
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

    /// Lists every outgoing related record id reachable through `relation`.
    async fn outgoing_ids(&self, relation: &str) -> Result<Vec<RecordId>>
    where
        Self: ResolveRecordId + Sync,
    {
        crate::graph::outgoing_ids(self.resolve_record_id().await?, relation).await
    }

    /// Loads outgoing related records of type `T` reachable through `relation`.
    async fn outgoing<T>(&self, relation: &str) -> Result<Vec<T>>
    where
        Self: ResolveRecordId + Sync,
        T: ModelMeta + StoredModel + ForeignModel,
        T::Stored: serde::de::DeserializeOwned,
    {
        crate::graph::outgoing::<T>(self.resolve_record_id().await?, relation).await
    }

    /// Counts every outgoing edge reachable through `relation`.
    async fn outgoing_count(&self, relation: &str) -> Result<i64>
    where
        Self: ResolveRecordId + Sync,
    {
        crate::graph::outgoing_count(self.resolve_record_id().await?, relation).await
    }

    /// Counts outgoing related records of type `T` reachable through `relation`.
    async fn outgoing_count_as<T>(&self, relation: &str) -> Result<i64>
    where
        Self: ResolveRecordId + Sync,
        T: ModelMeta + StoredModel + ForeignModel,
    {
        crate::graph::outgoing_count_as::<T>(self.resolve_record_id().await?, relation).await
    }

    /// Lists every incoming related record id that points to `self` through `relation`.
    async fn incoming_ids(&self, relation: &str) -> Result<Vec<RecordId>>
    where
        Self: ResolveRecordId + Sync,
    {
        crate::graph::incoming_ids(self.resolve_record_id().await?, relation).await
    }

    /// Loads incoming related records of type `T` that point to `self` through `relation`.
    async fn incoming<T>(&self, relation: &str) -> Result<Vec<T>>
    where
        Self: ResolveRecordId + Sync,
        T: ModelMeta + StoredModel + ForeignModel,
        T::Stored: serde::de::DeserializeOwned,
    {
        crate::graph::incoming::<T>(self.resolve_record_id().await?, relation).await
    }

    /// Counts every incoming edge that points to `self` through `relation`.
    async fn incoming_count(&self, relation: &str) -> Result<i64>
    where
        Self: ResolveRecordId + Sync,
    {
        crate::graph::incoming_count(self.resolve_record_id().await?, relation).await
    }

    /// Counts incoming related records of type `T` that point to `self` through `relation`.
    async fn incoming_count_as<T>(&self, relation: &str) -> Result<i64>
    where
        Self: ResolveRecordId + Sync,
        T: ModelMeta + StoredModel + ForeignModel,
    {
        crate::graph::incoming_count_as::<T>(self.resolve_record_id().await?, relation).await
    }

    /// Returns whether the model table currently contains at least one row.
    async fn exists() -> Result<bool> {
        Repo::<Self>::exists().await
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
