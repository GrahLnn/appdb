use std::marker::PhantomData;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use surrealdb::opt::PatchOp;
use surrealdb::types::{RecordId, RecordIdKey, Table, Value as SurrealDbValue};

use crate::connection::get_db;
use crate::error::DBError;
use crate::model::meta::{HasId, ModelMeta};
use crate::query::builder::QueryKind;

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

fn prepare_save_parts<T>(data: T) -> Result<(RecordId, Value, Value)>
where
    T: ModelMeta,
{
    let table = T::table_name();
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

fn normalize_row_with_id<T>(row: SurrealDbValue, id: Value) -> Result<T>
where
    T: ModelMeta,
{
    let mut row = row.into_json_value();
    match &mut row {
        Value::Object(map) => {
            map.insert("id".to_owned(), id);
            Ok(serde_json::from_value(row)?)
        }
        _ => Err(DBError::InvalidModel(format!(
            "database returned non-object row for `{}`",
            std::any::type_name::<T>()
        ))
        .into()),
    }
}

/// Generic repository over a model type registered with [`ModelMeta`].
/// Generic repository helpers for a model type.
pub struct Repo<T>(PhantomData<T>);

impl<T> Repo<T>
where
    T: ModelMeta,
{
    /// Creates a new row in the model table.
    /// Creates a new row in the model table.
    pub async fn create(data: T) -> Result<T> {
        let db = get_db()?;
        let created: Option<T> = db.create(T::table_name()).content(data).await?;
        created.ok_or(DBError::EmptyResult("create").into())
    }

    /// Creates a new row and returns only its record id.
    /// Creates a new row and returns its record id.
    pub async fn create_return_id(data: T) -> Result<RecordId> {
        let db = get_db()?;
        let created: Option<RecordId> = db
            .query(QueryKind::create_return_id(T::table_name()))
            .bind(("table", Table::from(T::table_name())))
            .bind(("data", data))
            .await?
            .check()?
            .take(0)?;
        created.ok_or(DBError::EmptyResult("create_return_id").into())
    }

    /// Creates a row with an explicit record id key.
    /// Creates a new row at an explicit id key.
    pub async fn create_by_id<K>(id: K, data: T) -> Result<T>
    where
        RecordIdKey: From<K>,
        K: Send,
    {
        let db = get_db()?;
        let created: Option<T> = db.create((T::table_name(), id)).content(data).await?;
        created.ok_or(DBError::EmptyResult("create_by_id").into())
    }

    /// Upserts a row using [`HasId::id`] as the record id.
    /// Upserts a row using the record id exposed by `HasId`.
    pub async fn upsert(data: T) -> Result<T>
    where
        T: HasId,
    {
        let db = get_db()?;
        let updated: Option<T> = db.upsert(data.id()).content(data).await?;
        updated.ok_or(DBError::EmptyResult("upsert").into())
    }

    /// Upserts a row into an explicit record id.
    /// Upserts a row at the provided record id.
    pub async fn upsert_by_id(id: RecordId, data: T) -> Result<T> {
        let db = get_db()?;
        let updated: Option<T> = db.upsert(id).content(data).await?;
        updated.ok_or(DBError::EmptyResult("upsert_by_id").into())
    }

    /// Fetches a row by the model table plus raw id key.
    /// Loads a row by table-local id key.
    pub async fn get_by_key<K>(id: K) -> Result<T>
    where
        RecordIdKey: From<K>,
        K: Send,
    {
        let db = get_db()?;
        let record: Option<T> = db.select((T::table_name(), id)).await?;
        record.ok_or(DBError::NotFound.into())
    }

    /// Fetches a row by full record id.
    /// Loads a row by full `RecordId`.
    pub async fn get_record(record: RecordId) -> Result<T> {
        let db = get_db()?;
        let record: Option<T> = db.select(record).await?;
        record.ok_or(DBError::NotFound.into())
    }

    /// Returns every row in the table.
    /// Reads the entire table as typed rows.
    pub async fn scan() -> Result<Vec<T>> {
        let db = get_db()?;
        let records: Vec<T> = db.select(T::table_name()).await?;
        Ok(records)
    }

    /// Returns up to `count` rows from the table.
    /// Reads up to `count` rows from the table.
    pub async fn scan_limit(count: i64) -> Result<Vec<T>> {
        let db = get_db()?;
        let records: Vec<T> = db
            .query(QueryKind::limit(T::table_name(), count))
            .bind(("table", Table::from(T::table_name())))
            .bind(("count", count))
            .await?
            .check()?
            .take(0)?;
        Ok(records)
    }

    /// Replaces the stored row at `id` with `data`.
    /// Replaces the stored content of a row by id.
    pub async fn update_by_id(id: RecordId, data: T) -> Result<T> {
        let db = get_db()?;
        let updated: Option<T> = db.update(id).content(data).await?;
        updated.ok_or(DBError::NotFound.into())
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
        let created: Vec<T> = db.insert(T::table_name()).content(data).await?;
        Ok(created)
    }

    /// Bulk-inserts rows while ignoring conflicting duplicates.
    /// Inserts many rows while ignoring duplicate-key conflicts.
    pub async fn insert_ignore(data: Vec<T>) -> Result<Vec<T>> {
        let db = get_db()?;
        let chunk_size = 50_000;
        let mut inserted_all = Vec::with_capacity(data.len());

        for chunk in data.chunks(chunk_size) {
            let chunk_clone = chunk.to_vec();
            let inserted: Vec<T> = db
                .query(QueryKind::insert(T::table_name()))
                .bind(("table", Table::from(T::table_name())))
                .bind(("data", chunk_clone))
                .await?
                .check()?
                .take(0)?;
            inserted_all.extend(inserted);
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
            let chunk_clone = chunk.to_vec();
            let inserted: Vec<T> = db
                .query(QueryKind::insert_or_replace(T::table_name(), keys.clone()))
                .bind(("table", Table::from(T::table_name())))
                .bind(("data", chunk_clone))
                .await?
                .check()?
                .take(0)?;
            inserted_all.extend(inserted);
        }

        Ok(inserted_all)
    }

    /// Deletes one row by raw id key.
    /// Deletes a row by table-local id key.
    pub async fn delete_by_key<K>(id: K) -> Result<()>
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

    /// Finds the first record id where field `k` equals `v`.
    /// Finds the first record id matching a field equality filter.
    pub async fn find_record_id(k: &str, v: &str) -> Result<RecordId> {
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
}

impl<T> Repo<T>
where
    T: ModelMeta,
{
    /// Upserts one model using its `id` field and returns the normalized row.
    /// Saves a model by its `id` field and returns the normalized row.
    pub async fn save(data: T) -> Result<T> {
        let db = get_db()?;
        let (record, content, id) = prepare_save_parts(data)?;
        let row: Option<SurrealDbValue> = db.upsert(record).content(content).await?;
        let row = row.ok_or(DBError::EmptyResult("save"))?;
        normalize_row_with_id(row, id)
    }

    /// Fetches one model by raw id key and normalizes the returned `id`.
    /// Loads a row by its `id` field using the normalized query path.
    pub async fn get<K>(id: K) -> Result<T>
    where
        RecordIdKey: From<K>,
        K: Send,
    {
        let db = get_db()?;
        let record = RecordId::new(T::table_name(), id);
        let mut result = db
            .query(QueryKind::select_by_id())
            .bind(("record", record))
            .await?
            .check()?;
        let row: Option<T> = result.take(0)?;
        row.ok_or(DBError::NotFound.into())
    }

    /// Lists all rows with a normalized `id` field.
    /// Lists all rows with normalized `id` values.
    pub async fn list() -> Result<Vec<T>> {
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::select_all_with_id())
            .bind(("table", Table::from(T::table_name())))
            .await?
            .check()?;
        let rows: Vec<T> = result.take(0)?;
        Ok(rows)
    }

    /// Lists up to `count` rows with a normalized `id` field.
    /// Lists up to `count` rows with normalized `id` values.
    pub async fn list_limit(count: i64) -> Result<Vec<T>> {
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::select_limit_with_id())
            .bind(("table", Table::from(T::table_name())))
            .bind(("count", count))
            .await?
            .check()?;
        let rows: Vec<T> = result.take(0)?;
        Ok(rows)
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
                let (record, content, id) = prepare_save_parts(row)?;
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
                let inserted = normalize_row_with_id(row, id)?;
                inserted_all.push(inserted);
            }
        }

        Ok(inserted_all)
    }
}

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

    crate::impl_crud!(AutoTableModel);
    crate::impl_crud!(CustomTableModel, "custom_users");

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

/// Trait facade that forwards model methods to [`Repo`].
#[async_trait]
/// Convenience trait that forwards CRUD calls to `Repo<Self>`.
pub trait Crud: ModelMeta {
    /// Builds a record id for the model table.
    /// Builds a full record id for this model table.
    fn record_id<T>(id: T) -> RecordId
    where
        RecordIdKey: From<T>,
    {
        <Self as ModelMeta>::record_id(id)
    }

    /// Creates the current value as a new row.
    /// Creates a copy of `self` in the database.
    async fn create(&self) -> Result<Self> {
        Repo::<Self>::create(self.clone()).await
    }

    /// Creates the current value and returns only the record id.
    /// Creates a copy of `self` and returns its record id.
    async fn create_return_id(&self) -> Result<RecordId> {
        Repo::<Self>::create_return_id(self.clone()).await
    }

    /// Creates a row using an explicit id key.
    /// Creates a row at an explicit id key.
    async fn create_by_id<T>(id: T, data: Self) -> Result<Self>
    where
        RecordIdKey: From<T>,
        T: Send,
    {
        Repo::<Self>::create_by_id(id, data).await
    }

    /// Upserts the current value using [`HasId::id`].
    /// Upserts `self` using its `HasId` implementation.
    async fn upsert(&self) -> Result<Self>
    where
        Self: HasId,
    {
        Repo::<Self>::upsert(self.clone()).await
    }

    /// Upserts `data` into an explicit record id.
    /// Upserts `data` at the provided record id.
    async fn upsert_by_id(id: RecordId, data: Self) -> Result<Self> {
        Repo::<Self>::upsert_by_id(id, data).await
    }

    /// Fetches one row by raw id key.
    /// Loads a row by table-local id key.
    async fn get_by_key<T>(id: T) -> Result<Self>
    where
        RecordIdKey: From<T>,
        T: Send,
    {
        Repo::<Self>::get_by_key(id).await
    }

    /// Fetches one row by full record id.
    /// Loads a row by full `RecordId`.
    async fn get_record(record: RecordId) -> Result<Self> {
        Repo::<Self>::get_record(record).await
    }

    /// Returns every row in the model table.
    /// Reads the entire table.
    async fn scan() -> Result<Vec<Self>> {
        Repo::<Self>::scan().await
    }

    /// Lists all rows with a normalized `id` field.
    /// Lists all rows with normalized `id` values.
    async fn list() -> Result<Vec<Self>> {
        Repo::<Self>::list().await
    }

    /// Lists up to `count` rows with a normalized `id` field.
    /// Lists up to `count` rows with normalized `id` values.
    async fn list_limit(count: i64) -> Result<Vec<Self>> {
        Repo::<Self>::list_limit(count).await
    }

    /// Updates the current value at its record id.
    /// Replaces the stored content of `self`.
    async fn update(self) -> Result<Self>
    where
        Self: HasId,
    {
        Repo::<Self>::update_by_id(self.id(), self).await
    }

    /// Updates `self` at an explicit record id.
    /// Replaces the stored content of `self` at the provided id.
    async fn update_by_id(self, id: RecordId) -> Result<Self> {
        Repo::<Self>::update_by_id(id, self).await
    }

    /// Merges a partial JSON object into a row.
    /// Merges a partial JSON object into an existing row.
    async fn merge(id: RecordId, data: Value) -> Result<Self> {
        Repo::<Self>::merge(id, data).await
    }

    /// Applies patch operations to a row.
    /// Applies SurrealDB patch operations to an existing row.
    async fn patch(id: RecordId, data: Vec<PatchOp>) -> Result<Self> {
        Repo::<Self>::patch(id, data).await
    }

    /// Bulk-inserts rows.
    /// Inserts many rows using SurrealDB bulk insert.
    async fn insert(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::insert(data).await
    }

    /// Bulk-inserts rows while ignoring duplicate conflicts.
    /// Inserts many rows while ignoring duplicate-key conflicts.
    async fn insert_ignore(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::insert_ignore(data).await
    }

    /// Bulk-inserts rows and updates conflicting rows.
    /// Inserts many rows and updates existing rows on duplicate key.
    async fn insert_or_replace(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::insert_or_replace(data).await
    }

    /// Deletes the current row.
    /// Deletes `self` by its record id.
    async fn delete(self) -> Result<()>
    where
        Self: HasId,
    {
        Repo::<Self>::delete_record(self.id()).await
    }

    /// Deletes one row by raw id key.
    /// Deletes a row by table-local id key.
    async fn delete_by_key<T>(id: T) -> Result<()>
    where
        RecordIdKey: From<T>,
        T: Send,
    {
        Repo::<Self>::delete_by_key(id).await
    }

    /// Deletes one row by full record id.
    /// Deletes a row by full `RecordId`.
    async fn delete_record(id: RecordId) -> Result<()> {
        Repo::<Self>::delete_record(id).await
    }

    /// Deletes all rows from the model table.
    /// Deletes every row in the model table.
    async fn delete_all() -> Result<()> {
        Repo::<Self>::delete_all().await
    }

    /// Finds the first matching record id by field equality.
    /// Finds the first record id matching a field equality filter.
    async fn find_record_id(k: &str, v: &str) -> Result<RecordId> {
        Repo::<Self>::find_record_id(k, v).await
    }

    /// Lists all record ids in the model table.
    /// Lists all record ids in the model table.
    async fn list_record_ids() -> Result<Vec<RecordId>> {
        Repo::<Self>::list_record_ids().await
    }

    /// Upserts the current value by its `id` field.
    /// Saves `self` using its `id` field and returns the normalized row.
    async fn save(self) -> Result<Self> {
        Repo::<Self>::save(self).await
    }

    /// Fetches one row by raw id key and normalized `id`.
    /// Loads a row by its `id` field.
    async fn get<T>(id: T) -> Result<Self>
    where
        RecordIdKey: From<T>,
        T: Send,
    {
        Repo::<Self>::get(id).await
    }

    /// Batch-upserts rows by their `id` field.
    /// Saves many rows in chunks and returns normalized results.
    async fn save_many(data: Vec<Self>) -> Result<Vec<Self>> {
        Repo::<Self>::save_many(data).await
    }
}

#[macro_export]
/// Implements [`ModelMeta`] and [`Crud`] for a model type.
macro_rules! impl_crud {
    ($t:ty) => {
        impl $crate::model::meta::ModelMeta for $t {
            fn table_name() -> &'static str {
                static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
                TABLE_NAME.get_or_init(|| {
                    let table = $crate::model::meta::default_table_name(stringify!($t));
                    $crate::model::meta::register_table(stringify!($t), table)
                })
            }
        }

        impl $crate::repository::Crud for $t {}
    };
    ($t:ty, $table:expr) => {
        impl $crate::model::meta::ModelMeta for $t {
            fn table_name() -> &'static str {
                static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
                TABLE_NAME.get_or_init(|| {
                    let table: &'static str = $table;
                    $crate::model::meta::register_table(stringify!($t), table)
                })
            }
        }

        impl $crate::repository::Crud for $t {}
    };
}
