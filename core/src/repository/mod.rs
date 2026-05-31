use std::{marker::PhantomData, pin::Pin};

mod relation_sync;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use surrealdb::opt::PatchOp;
use surrealdb::types::{RecordId, RecordIdKey, SurrealValue, Table, Value as SurrealDbValue};

use crate::connection::get_db;
use crate::error::{DBError, DBErrorKind, classify_db_error_text};
use crate::model::meta::{
    HasId, ModelMeta, PaginationMeta, ResolveRecordId, UniqueLookupMeta, ViewMeta, ViewParams,
    ViewSource,
};
use crate::pagination::PaginationPlan;
use crate::query::builder::{Order, QueryKind};
use crate::query::{RawSqlStmt, query_bound, query_bound_checked};
use crate::serde_utils::id::parse_record_id_or_plain_string;
use crate::{ForeignModel, ForeignWritePlan, StoredModel};

pub use crate::pagination::{Page, PageCursor};
use relation_sync::{
    append_relation_sync_to_stmt, append_relation_sync_with_anchor_expr_to_stmt,
    ensure_relation_tables,
};

fn struct_field_names<T: Serialize>(data: &T) -> Result<Vec<String>> {
    let value = serde_json::to_value(data)?;
    match value {
        Value::Object(map) => {
            let mut fields = Vec::with_capacity(map.len());
            for key in map.keys() {
                if !is_plain_surreal_identifier(key) {
                    return Err(DBError::InvalidIdentifier(format!(
                        "insert_or_replace field `{key}` must be a plain SurrealQL identifier"
                    ))
                    .into());
                }
                fields.push(key.clone());
            }
            Ok(fields)
        }
        _ => Ok(vec![]),
    }
}

fn is_plain_surreal_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    matches!(first, b'A'..=b'Z' | b'a'..=b'z' | b'_')
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn strip_null_db_fields(value: &mut SurrealDbValue) {
    match value {
        SurrealDbValue::Object(map) => {
            let null_keys = map
                .iter()
                .filter_map(|(key, value)| {
                    if value.is_null() || value.is_none() {
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
                strip_null_db_fields(nested);
            }
        }
        SurrealDbValue::Array(items) => {
            for nested in items.iter_mut() {
                strip_null_db_fields(nested);
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
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(|value| RecordId::new(T::storage_table(), value))
            .ok_or_else(|| {
                DBError::Decode(format!(
                    "stored row contains unsupported numeric id value `{value}`"
                ))
                .into()
            }),
        serde_json::Value::Object(_) => Ok(serde_json::from_value(id)?),
        other => Err(DBError::Decode(format!(
            "stored row contains unsupported id shape `{other}`"
        ))
        .into()),
    }
}

fn prepare_save_parts<M, T>(table: &str, data: T) -> Result<(RecordId, SurrealDbValue, Value)>
where
    T: Serialize + SurrealValue,
    M: ForeignModel,
{
    let key = extract_record_id_key(&data)?;
    let id = record_id_key_to_json_value(&key);
    let record = RecordId::new(table, key);
    Ok((record, prepare_content::<M, _>(data)?, id))
}

fn prepare_content<M, T>(data: T) -> Result<SurrealDbValue>
where
    T: SurrealValue,
    M: ForeignModel,
{
    let mut content = data.into_value();
    if let SurrealDbValue::Object(map) = &mut content {
        map.remove("id");
        for field in M::relation_field_names() {
            map.remove(*field);
        }
    }
    strip_null_db_fields(&mut content);
    Ok(content)
}

fn prepare_create_content<M, T>(data: T) -> Result<SurrealDbValue>
where
    T: SurrealValue,
    M: ForeignModel,
{
    let mut content = data.into_value();
    if let SurrealDbValue::Object(map) = &mut content {
        for field in M::relation_field_names() {
            map.remove(*field);
        }
    }
    strip_null_db_fields(&mut content);
    Ok(content)
}

#[derive(Clone, Copy)]
enum ExplicitWriteMode {
    CreateOnly,
    Upsert,
    Update,
}

impl ExplicitWriteMode {
    fn write_sql(self) -> &'static str {
        match self {
            Self::CreateOnly => "CREATE ONLY $record CONTENT $data RETURN AFTER;",
            Self::Upsert => "UPSERT ONLY $record CONTENT $data RETURN AFTER;",
            Self::Update => "UPDATE $record CONTENT $data RETURN AFTER;",
        }
    }

    fn map_error(self, err: DBError) -> DBError {
        match self {
            Self::CreateOnly if matches!(err, DBError::EmptyResult(_)) => {
                DBError::Conflict("record already exists".to_owned())
            }
            _ => err,
        }
    }

    fn empty_result_error(self) -> DBError {
        match self {
            Self::CreateOnly => DBError::Conflict("record already exists".to_owned()),
            Self::Upsert => DBError::EmptyResult("persist_explicit_id_primitive"),
            Self::Update => DBError::NotFound,
        }
    }
}

async fn decode_write_return_view<V>(row: SurrealDbValue, id: RecordId) -> Result<V>
where
    V: ViewMeta,
{
    let mut value = row.into_json_value();
    if let Value::Object(map) = &mut value {
        map.insert("id".to_owned(), serde_json::to_value(id)?);
    }
    decode_view_row::<V>(value).await
}

async fn persist_explicit_id_primitive<T>(record: RecordId, data: T, create_only: bool) -> Result<T>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    persist_explicit_id_primitive_with_foreign_plan::<T>(
        record,
        data,
        create_only,
        &ForeignWritePlan::new(),
    )
    .await
}

async fn write_explicit_id_primitive_with_foreign_plan<T>(
    record: RecordId,
    data: T,
    mode: ExplicitWriteMode,
    foreign_plan: &ForeignWritePlan,
) -> Result<(SurrealDbValue, T)>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    let original = data.clone();
    let stored_input = T::persist_foreign_with_plan(data, foreign_plan).await?;
    let content = prepare_content::<T, _>(stored_input)?;
    let relation_writes = original.prepare_relation_writes(record.clone()).await?;
    ensure_relation_tables(&relation_writes).await?;
    let mut stmt = RawSqlStmt::new("BEGIN TRANSACTION;");
    stmt.sql.push_str(mode.write_sql());
    stmt = stmt.bind("record", record.clone()).bind("data", content);
    let (stmt_with_relations, _) = append_relation_sync_to_stmt(stmt, &relation_writes, "rel")?;
    let mut stmt = stmt_with_relations;
    stmt.sql.push_str("COMMIT TRANSACTION;");

    let result = query_bound(stmt).await;
    let mut result = match result {
        Ok(result) => result,
        Err(err) => {
            let typed = DBError::from(err);
            return Err(mode.map_error(typed).into());
        }
    };
    result = match result.check() {
        Ok(result) => result,
        Err(err) => {
            let typed = DBError::from(err);
            return Err(mode.map_error(typed).into());
        }
    };

    let row: Option<SurrealDbValue> = result.take(1)?;
    let row = row.ok_or_else(|| mode.empty_result_error())?;
    Ok((row, original))
}

async fn persist_explicit_id_primitive_with_foreign_plan<T>(
    record: RecordId,
    data: T,
    create_only: bool,
    foreign_plan: &ForeignWritePlan,
) -> Result<T>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    let mode = if create_only {
        ExplicitWriteMode::CreateOnly
    } else {
        ExplicitWriteMode::Upsert
    };
    let (row, original) = write_explicit_id_primitive_with_foreign_plan::<T>(
        record.clone(),
        data,
        mode,
        foreign_plan,
    )
    .await?;
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

async fn collect_lookup_parts<T>(data: &T) -> Result<Vec<(String, SurrealDbValue)>>
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
        let value = match data.resolve_lookup_field_value(field).await? {
            Some(value) => value,
            None => map
                .get(*field)
                .cloned()
                .ok_or_else(|| {
                    DBError::InvalidModel(format!(
                        "model `{}` is missing lookup field `{field}` during automatic unique lookup",
                        std::any::type_name::<T>()
                    ))
                })?
                .into_value(),
        };
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

fn decode_view_row_error<V>(row: Value, err: anyhow::Error) -> anyhow::Error
where
    V: ViewMeta,
{
    let classified = classify_db_error_text(format!(
        "failed to decode view `{}` over `{}` row: {err}; row={row}",
        std::any::type_name::<V>(),
        V::source_table()
    ));
    debug_assert_eq!(classified.kind, DBErrorKind::Decode);
    classified.into_db_error().into()
}

async fn raw_rows_to_views<V>(rows: Vec<SurrealDbValue>) -> Result<Vec<V>>
where
    V: ViewMeta,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(decode_view_row::<V>(row.into_json_value()).await?);
    }
    Ok(values)
}

async fn raw_rows_to_view_records<V>(rows: Vec<SurrealDbValue>) -> Result<Vec<ViewRecord<V>>>
where
    V: ViewMeta,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(decode_view_record::<V>(row.into_json_value()).await?);
    }
    Ok(values)
}

async fn raw_rows_to_view_related_records<V>(
    rows: Vec<SurrealDbValue>,
) -> Result<Vec<ViewRelatedRecord<V>>>
where
    V: ViewMeta,
{
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(decode_view_related_record::<V>(row.into_json_value()).await?);
    }
    Ok(values)
}

fn exactly_one_lookup_id<M>(ids: Vec<RecordId>, field: &str, value: &str) -> Result<RecordId> {
    match ids.len() {
        0 => Err(DBError::NotFound.into()),
        1 => Ok(ids
            .into_iter()
            .next()
            .expect("length checked before single lookup id extraction")),
        _ => Err(DBError::InvalidModel(format!(
            "`find_one` for `{}` by `{field}` = `{value}` matched multiple records",
            std::any::type_name::<M>()
        ))
        .into()),
    }
}

async fn decode_view_row<V>(mut row: Value) -> Result<V>
where
    V: ViewMeta,
{
    if let Value::Object(map) = &mut row {
        for field in V::nested_view_fields() {
            if let Some(value) = map.get_mut(*field) {
                crate::decode_stored_record_links(value);
            }
        }
    }
    normalize_public_output_ids(&mut row);
    let stored = V::decode_stored_view_row(row.clone())
        .map_err(|err| decode_view_row_error::<V>(row, err))?;
    V::hydrate_view(stored).await
}

async fn decode_view_record<V>(mut row: Value) -> Result<ViewRecord<V>>
where
    V: ViewMeta,
{
    let record = match row
        .as_object_mut()
        .and_then(|map| map.remove("__appdb_record"))
    {
        Some(Value::String(text)) => {
            parse_record_id_or_plain_string(&text, Some(V::source_table())).map_err(|invalid| {
                DBError::Decode(format!(
                    "view row contains invalid source record id `{invalid}`"
                ))
            })?
        }
        Some(value) => serde_json::from_value(value)?,
        None => {
            return Err(DBError::Decode("view row is missing source record id".to_owned()).into());
        }
    };
    let value = decode_view_row::<V>(row).await?;
    Ok(ViewRecord { id: record, value })
}

async fn decode_view_related_record<V>(mut row: Value) -> Result<ViewRelatedRecord<V>>
where
    V: ViewMeta,
{
    let owner = match row
        .as_object_mut()
        .and_then(|map| map.remove("__appdb_owner"))
    {
        Some(Value::String(text)) => {
            parse_record_id_or_plain_string(&text, None).map_err(|invalid| {
                DBError::Decode(format!(
                    "view relation row contains invalid owner id `{invalid}`"
                ))
            })?
        }
        Some(value) => serde_json::from_value(value)?,
        None => {
            return Err(DBError::Decode("view relation row is missing owner id".to_owned()).into());
        }
    };
    let record = decode_view_record::<V>(row).await?;
    Ok(ViewRelatedRecord { owner, record })
}

/// Lazy list query surface that can either execute immediately or be refined
/// into an ordered scan before awaiting.
#[must_use = "list queries do nothing until awaited"]
pub struct ListQuery<T>(PhantomData<T>);

impl<T> ListQuery<T> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> ListQuery<T>
where
    T: ModelMeta + StoredModel + ForeignModel + PaginationMeta,
{
    /// Orders the full list by `id` or the declared `#[pagin]` field.
    pub fn order_by(self, field: impl Into<String>, order: Order) -> OrderedListQuery<T> {
        OrderedListQuery {
            field: field.into(),
            order,
            _marker: PhantomData,
        }
    }
}

impl<T> std::future::IntoFuture for ListQuery<T>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    type Output = Result<Vec<T>>;
    type IntoFuture = Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { Repo::<T>::list_all().await })
    }
}

/// Ordered full-table scan produced from [`ListQuery::order_by`].
#[must_use = "ordered list queries do nothing until awaited"]
pub struct OrderedListQuery<T> {
    field: String,
    order: Order,
    _marker: PhantomData<T>,
}

impl<T> std::future::IntoFuture for OrderedListQuery<T>
where
    T: ModelMeta + StoredModel + ForeignModel + PaginationMeta,
{
    type Output = Result<Vec<T>>;
    type IntoFuture = Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { Repo::<T>::list_ordered(&self.field, self.order).await })
    }
}

/// Lazy read-only View list query surface.
#[must_use = "view list queries do nothing until awaited"]
pub struct ViewListQuery<V>(PhantomData<V>);

impl<V> ViewListQuery<V> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

impl<V> ViewListQuery<V>
where
    V: ViewMeta,
{
    /// Orders a View list by `id`, a declared view field, or the source `#[pagin]` field.
    pub fn order_by(self, field: impl Into<String>, order: Order) -> ViewOrderedListQuery<V> {
        ViewOrderedListQuery {
            field: field.into(),
            order,
            _marker: PhantomData,
        }
    }
}

/// Read-only View value paired with the source record that produced it.
///
/// This is appdb-owned evidence for composing View reads across relations. It
/// keeps record identity out of domain models while still letting callers ask
/// for related View projections without re-identifying rows through business
/// fields.
#[derive(Debug, Clone)]
pub struct ViewRecord<V> {
    id: RecordId,
    value: V,
}

impl<V> ViewRecord<V> {
    pub fn id(&self) -> &RecordId {
        &self.id
    }

    pub fn into_id(self) -> RecordId {
        self.id
    }

    pub fn value(&self) -> &V {
        &self.value
    }

    pub fn into_value(self) -> V {
        self.value
    }
}

impl<V> std::ops::Deref for ViewRecord<V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

/// Read-only View relation result paired with the relation owner that produced it.
///
/// The owner is the relation anchor supplied to the batch query: the `in`
/// record for outgoing queries, or the `out` record for incoming queries.
#[derive(Debug, Clone)]
pub struct ViewRelatedRecord<V> {
    owner: RecordId,
    record: ViewRecord<V>,
}

impl<V> ViewRelatedRecord<V> {
    pub fn owner(&self) -> &RecordId {
        &self.owner
    }

    pub fn record(&self) -> &ViewRecord<V> {
        &self.record
    }

    pub fn into_parts(self) -> (RecordId, ViewRecord<V>) {
        (self.owner, self.record)
    }
}

impl<V> std::future::IntoFuture for ViewListQuery<V>
where
    V: ViewMeta,
{
    type Output = Result<Vec<V>>;
    type IntoFuture = Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { ViewRepo::<V>::list_all().await })
    }
}

/// Ordered projected scan produced from [`ViewListQuery::order_by`].
#[must_use = "ordered view list queries do nothing until awaited"]
pub struct ViewOrderedListQuery<V> {
    field: String,
    order: Order,
    _marker: PhantomData<V>,
}

impl<V> std::future::IntoFuture for ViewOrderedListQuery<V>
where
    V: ViewMeta,
{
    type Output = Result<Vec<V>>;
    type IntoFuture = Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { ViewRepo::<V>::list_ordered(&self.field, self.order).await })
    }
}

/// Marker for View projections that can be returned after writing their Store source.
pub trait WriteReturnView<T>: ViewMeta<Source = T>
where
    T: ModelMeta + PaginationMeta,
{
}

impl<T, V> WriteReturnView<T> for V
where
    T: ModelMeta + PaginationMeta,
    V: ViewMeta<Source = T>,
{
}

/// Internal repository building blocks for read-only View projections.
pub struct ViewRepo<V>(PhantomData<V>);

impl<V> ViewRepo<V>
where
    V: ViewMeta,
{
    fn ensure_table_source(operation: &str) -> Result<()> {
        if V::source_kind() == ViewSource::Table {
            return Ok(());
        }

        Err(DBError::InvalidModel(format!(
            "view `{}` does not support {operation} because it uses a custom SQL source",
            std::any::type_name::<V>()
        ))
        .into())
    }

    fn validate_view_order_field(field: &str) -> Result<&str> {
        Self::ensure_table_source("table ordered list operations")?;

        if field == "id" || V::view_fields().contains(&field) {
            return Ok(field);
        }

        match V::source_pagination_field() {
            Some(pagination_field) if pagination_field == field => Ok(field),
            Some(pagination_field) => Err(DBError::InvalidModel(format!(
                "view `{}` only supports ordered list by `id`, declared view fields, or source #[pagin] field `{pagination_field}`, got `{field}`",
                std::any::type_name::<V>()
            ))
            .into()),
            None => Err(DBError::InvalidModel(format!(
                "view `{}` only supports ordered list by `id` or declared view fields, got `{field}`",
                std::any::type_name::<V>()
            ))
            .into()),
        }
    }

    /// Starts a projected list query that can be ordered before awaiting.
    pub fn list() -> ViewListQuery<V> {
        ViewListQuery::new()
    }

    /// Lists rows from this View's custom SQL source.
    pub async fn query(params: V::Params) -> Result<Vec<V>> {
        let sql = V::sql().ok_or_else(|| {
            DBError::InvalidModel(format!(
                "view `{}` does not declare a custom SQL source",
                std::any::type_name::<V>()
            ))
        })?;
        let stmt = V::Params::bind_view_params(params, RawSqlStmt::new(sql))?;
        let rows: Vec<SurrealDbValue> =
            crate::query::query_bound_take(stmt, Some(V::sql_result_index())).await?;
        raw_rows_to_views::<V>(rows).await
    }

    /// Lists every row projected to the View's declared fields.
    async fn list_all() -> Result<Vec<V>> {
        Self::ensure_table_source("list() without query params")?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_all(V::view_fields()))
            .bind(("table", Table::from(V::source_table())))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(0)?;
        raw_rows_to_views::<V>(rows).await
    }

    /// Lists projected rows with source-record evidence.
    pub async fn list_records() -> Result<Vec<ViewRecord<V>>> {
        Self::ensure_table_source("list_records()")?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_all_with_record(V::view_fields()))
            .bind(("table", Table::from(V::source_table())))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(0)?;
        raw_rows_to_view_records::<V>(rows).await
    }

    /// Lists every projected row ordered by an allowed field.
    pub async fn list_ordered(field: &str, order: Order) -> Result<Vec<V>> {
        let field = Self::validate_view_order_field(field)?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_all_by_order(order, field, V::view_fields()))
            .bind(("table", Table::from(V::source_table())))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(1)?;
        raw_rows_to_views::<V>(rows).await
    }

    /// Fetches one projected row by table-local id key.
    pub async fn get<K>(id: K) -> Result<V>
    where
        RecordIdKey: From<K>,
        K: Send,
    {
        Self::ensure_table_source("get()")?;
        let record = RecordId::new(V::source_table(), id);
        Self::get_record(record).await
    }

    /// Fetches one projected row by full record id.
    pub async fn get_record(record: RecordId) -> Result<V> {
        Self::ensure_table_source("get_record()")?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_by_id(V::view_fields()))
            .bind(("record", record))
            .await?
            .check()?;
        let row: Option<SurrealDbValue> = result.take(0)?;
        match row {
            Some(row) => decode_view_row::<V>(row.into_json_value()).await,
            None => Err(DBError::NotFound.into()),
        }
    }

    /// Loads outgoing related rows projected as this View.
    pub async fn outgoing_records(record: RecordId, relation: &str) -> Result<Vec<ViewRecord<V>>> {
        Self::ensure_table_source("outgoing_records()")?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_outgoing(V::view_fields()))
            .bind(("rel", Table::from(relation)))
            .bind(("in", record))
            .bind(("out_table", V::source_table().to_owned()))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(1)?;
        raw_rows_to_view_records::<V>(rows).await
    }

    /// Loads outgoing related rows for many records, preserving each relation owner.
    pub async fn outgoing_records_by_owners(
        records: Vec<RecordId>,
        relation: &str,
    ) -> Result<Vec<ViewRelatedRecord<V>>> {
        Self::ensure_table_source("outgoing_records_by_owners()")?;
        if records.is_empty() {
            return Ok(vec![]);
        }

        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_outgoing_many(V::view_fields()))
            .bind(("rel", Table::from(relation)))
            .bind(("ins", records))
            .bind(("out_table", V::source_table().to_owned()))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(0)?;
        raw_rows_to_view_related_records::<V>(rows).await
    }

    /// Loads incoming related rows projected as this View.
    pub async fn incoming_records(record: RecordId, relation: &str) -> Result<Vec<ViewRecord<V>>> {
        Self::ensure_table_source("incoming_records()")?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_incoming(V::view_fields()))
            .bind(("rel", Table::from(relation)))
            .bind(("out", record))
            .bind(("in_table", V::source_table().to_owned()))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(1)?;
        raw_rows_to_view_records::<V>(rows).await
    }

    /// Loads incoming related rows for many records, preserving each relation owner.
    pub async fn incoming_records_by_owners(
        records: Vec<RecordId>,
        relation: &str,
    ) -> Result<Vec<ViewRelatedRecord<V>>> {
        Self::ensure_table_source("incoming_records_by_owners()")?;
        if records.is_empty() {
            return Ok(vec![]);
        }

        let db = get_db()?;
        let mut result = db
            .query(QueryKind::view_incoming_many(V::view_fields()))
            .bind(("rel", Table::from(relation)))
            .bind(("outs", records))
            .bind(("in_table", V::source_table().to_owned()))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(0)?;
        raw_rows_to_view_related_records::<V>(rows).await
    }

    /// Finds one matching source row and returns it as this View.
    pub async fn find_one(k: &str, v: &str) -> Result<V> {
        Self::ensure_table_source("find_one()")?;
        let id = Self::find_one_id(k, v).await?;
        Self::get_record(id).await
    }

    /// Finds exactly one source record id matching a field equality filter.
    pub async fn find_one_id(k: &str, v: &str) -> Result<RecordId> {
        Self::ensure_table_source("find_one_id()")?;
        let db = get_db()?;
        let ids: Vec<RecordId> = db
            .query(QueryKind::select_id_single(V::source_table()))
            .bind(("table", Table::from(V::source_table())))
            .bind(("k", k.to_owned()))
            .bind(("v", v.to_owned()))
            .await?
            .check()?
            .take(0)?;
        exactly_one_lookup_id::<V>(ids, k, v)
    }
}

/// One-shot write builder for replacing selected `#[foreign]` fields with
/// already-known record-id shapes.
#[must_use = "foreign writes do nothing until create_at/upsert_at/update_at is awaited"]
pub struct ForeignWriteQuery<T> {
    data: T,
    plan: ForeignWritePlan,
}

impl<T> ForeignWriteQuery<T> {
    pub fn new(data: T) -> Self {
        Self {
            data,
            plan: ForeignWritePlan::new(),
        }
    }

    pub fn set_field_shape<S>(
        mut self,
        field: &'static str,
        shape: S,
    ) -> Result<ForeignWriteQuery<T>>
    where
        S: Serialize,
    {
        self.plan.set_field_shape(field, shape)?;
        Ok(self)
    }
}

impl<T> ForeignWriteQuery<T>
where
    T: ModelMeta + StoredModel + ForeignModel + PaginationMeta,
{
    pub async fn create_at(self, id: RecordId) -> Result<T> {
        Repo::<T>::create_at_with_foreign_plan(id, self.data, &self.plan).await
    }

    pub async fn upsert_at(self, id: RecordId) -> Result<T> {
        Repo::<T>::upsert_at_with_foreign_plan(id, self.data, &self.plan).await
    }

    pub async fn update_at(self, id: RecordId) -> Result<T> {
        Repo::<T>::update_at_with_foreign_plan(id, self.data, &self.plan).await
    }

    pub async fn create_at_returning<V>(self, id: RecordId) -> Result<V>
    where
        V: WriteReturnView<T>,
    {
        Repo::<T>::create_at_with_foreign_plan_returning::<V>(id, self.data, &self.plan).await
    }

    pub async fn upsert_at_returning<V>(self, id: RecordId) -> Result<V>
    where
        V: WriteReturnView<T>,
    {
        Repo::<T>::upsert_at_with_foreign_plan_returning::<V>(id, self.data, &self.plan).await
    }

    pub async fn update_at_returning<V>(self, id: RecordId) -> Result<V>
    where
        V: WriteReturnView<T>,
    {
        Repo::<T>::update_at_with_foreign_plan_returning::<V>(id, self.data, &self.plan).await
    }
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
    fn ensure_raw_partial_update_supported() -> Result<()> {
        if T::supports_raw_partial_update() {
            return Ok(());
        }

        Err(DBError::InvalidModel(format!(
            "merge/patch is not supported for model `{}` because raw partial updates bypass Store field modifiers; use update_at, save, or upsert instead",
            std::any::type_name::<T>()
        ))
        .into())
    }

    fn ensure_raw_bulk_insert_supported(api: &str) -> Result<()> {
        if !T::has_foreign_fields() && !T::has_relation_fields() {
            return Ok(());
        }

        Err(DBError::InvalidModel(format!(
            "{api} is not supported for model `{}` because bulk insert cannot compose Store field modifiers with nested write effects; use save_many or per-row create/save instead",
            std::any::type_name::<T>()
        ))
        .into())
    }

    fn validate_list_order_field(field: &str) -> Result<&str>
    where
        T: PaginationMeta,
    {
        if field == "id" {
            return Ok(field);
        }

        match T::pagination_field() {
            Some(pagination_field) if pagination_field == field => Ok(field),
            Some(pagination_field) => Err(DBError::InvalidModel(format!(
                "model `{}` only supports ordered list by `id` or its #[pagin] field `{pagination_field}`, got `{field}`",
                std::any::type_name::<T>()
            ))
            .into()),
            None => Err(DBError::InvalidModel(format!(
                "model `{}` does not declare a #[pagin] field, so ordered list only supports `id`, got `{field}`",
                std::any::type_name::<T>()
            ))
            .into()),
        }
    }

    /// Creates a new row in the model table.
    /// Creates a new row in the model table.
    pub async fn create(data: T) -> Result<T> {
        if T::has_relation_fields() {
            let original = data.clone();
            let stored_input = T::persist_foreign(data).await?;
            let content = prepare_create_content::<T, _>(stored_input)?;
            let anchor_record = RecordId::new(T::storage_table(), "__appdb_pending_create__");
            let relation_writes = original.prepare_relation_writes(anchor_record).await?;
            ensure_relation_tables(&relation_writes).await?;
            let mut stmt = RawSqlStmt::new(
                "BEGIN TRANSACTION; LET $created = CREATE ONLY $table CONTENT $data RETURN AFTER;",
            );
            stmt = stmt
                .bind("table", Table::from(T::storage_table()))
                .bind("data", content);
            let (mut stmt, relation_statement_count) =
                append_relation_sync_with_anchor_expr_to_stmt(
                    stmt,
                    &relation_writes,
                    "rel",
                    "$created",
                )?;
            stmt.sql
                .push_str("SELECT *, record::id(id) AS id FROM ONLY $created;");
            stmt.sql.push_str("COMMIT TRANSACTION;");
            let mut result = query_bound_checked(stmt).await?;
            let row: Option<SurrealDbValue> = result.take(2 + relation_statement_count)?;
            let row = row.ok_or(DBError::EmptyResult("create"))?;
            let row_json = row.into_json_value();
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

    /// Creates a new row while replacing selected `#[foreign]` fields with
    /// already-known record-id shapes for this write only.
    pub async fn create_at_with_foreign_plan(
        id: RecordId,
        data: T,
        foreign_plan: &ForeignWritePlan,
    ) -> Result<T> {
        persist_explicit_id_primitive_with_foreign_plan::<T>(id, data, true, foreign_plan).await
    }

    /// Creates a new row with selected `#[foreign]` field overrides and returns a View.
    pub async fn create_at_with_foreign_plan_returning<V>(
        id: RecordId,
        data: T,
        foreign_plan: &ForeignWritePlan,
    ) -> Result<V>
    where
        T: PaginationMeta,
        V: WriteReturnView<T>,
    {
        let (row, _) = write_explicit_id_primitive_with_foreign_plan::<T>(
            id.clone(),
            data,
            ExplicitWriteMode::CreateOnly,
            foreign_plan,
        )
        .await?;
        decode_write_return_view::<V>(row, id).await
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

    /// Upserts a row while replacing selected `#[foreign]` fields with
    /// already-known record-id shapes for this write only.
    pub async fn upsert_at_with_foreign_plan(
        id: RecordId,
        data: T,
        foreign_plan: &ForeignWritePlan,
    ) -> Result<T> {
        persist_explicit_id_primitive_with_foreign_plan::<T>(id, data, false, foreign_plan).await
    }

    /// Upserts a row with selected `#[foreign]` field overrides and returns a View.
    pub async fn upsert_at_with_foreign_plan_returning<V>(
        id: RecordId,
        data: T,
        foreign_plan: &ForeignWritePlan,
    ) -> Result<V>
    where
        T: PaginationMeta,
        V: WriteReturnView<T>,
    {
        let (row, _) = write_explicit_id_primitive_with_foreign_plan::<T>(
            id.clone(),
            data,
            ExplicitWriteMode::Upsert,
            foreign_plan,
        )
        .await?;
        decode_write_return_view::<V>(row, id).await
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
            let original = data.clone();
            let stored_input = T::persist_foreign(data).await?;
            let content = prepare_content::<T, _>(stored_input)?;
            let relation_writes = original.prepare_relation_writes(id.clone()).await?;
            ensure_relation_tables(&relation_writes).await?;
            let mut stmt =
                RawSqlStmt::new("BEGIN TRANSACTION; UPDATE $record CONTENT $data RETURN AFTER;");
            stmt = stmt.bind("record", id.clone()).bind("data", content);
            let (stmt_with_relations, _) =
                append_relation_sync_to_stmt(stmt, &relation_writes, "rel")?;
            let mut stmt = stmt_with_relations;
            stmt.sql.push_str("COMMIT TRANSACTION;");
            let mut result = query_bound_checked(stmt).await?;
            let row: Option<SurrealDbValue> = result.take(1)?;
            let row = row.ok_or(DBError::NotFound)?;
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

    /// Replaces a row while using selected caller-provided foreign record-id shapes.
    pub async fn update_at_with_foreign_plan(
        id: RecordId,
        data: T,
        foreign_plan: &ForeignWritePlan,
    ) -> Result<T> {
        let (row, original) = write_explicit_id_primitive_with_foreign_plan::<T>(
            id.clone(),
            data,
            ExplicitWriteMode::Update,
            foreign_plan,
        )
        .await?;
        let stored = decode_saved_row_from_model::<T>(row, serde_json::to_value(id)?, &original)?;
        let mut value = serde_json::to_value(T::hydrate_foreign(stored).await?)?;
        normalize_public_output_ids(&mut value);
        Ok(serde_json::from_value(value)?)
    }

    /// Replaces a row with selected `#[foreign]` field overrides and returns a View.
    pub async fn update_at_with_foreign_plan_returning<V>(
        id: RecordId,
        data: T,
        foreign_plan: &ForeignWritePlan,
    ) -> Result<V>
    where
        T: PaginationMeta,
        V: WriteReturnView<T>,
    {
        let (row, _) = write_explicit_id_primitive_with_foreign_plan::<T>(
            id.clone(),
            data,
            ExplicitWriteMode::Update,
            foreign_plan,
        )
        .await?;
        decode_write_return_view::<V>(row, id).await
    }

    /// Merges a partial JSON object into the row at `id`.
    /// Merges a partial JSON object into an existing row.
    pub async fn merge(id: RecordId, data: Value) -> Result<T> {
        Self::ensure_raw_partial_update_supported()?;

        let db = get_db()?;
        let merged: Option<T> = db.update(id).merge(data).await?;
        merged.ok_or(DBError::NotFound.into())
    }

    /// Applies SurrealDB patch operations to the row at `id`.
    /// Applies SurrealDB patch operations to an existing row.
    pub async fn patch(id: RecordId, data: Vec<PatchOp>) -> Result<T> {
        Self::ensure_raw_partial_update_supported()?;

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
        Self::ensure_raw_bulk_insert_supported("insert")?;

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
        Self::ensure_raw_bulk_insert_supported("insert_ignore")?;

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
        Self::ensure_raw_bulk_insert_supported("insert_or_replace")?;

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

    /// Finds exactly one record id matching a field equality filter.
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
        exactly_one_lookup_id::<T>(ids, k, v)
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
        let lookup_parts = collect_lookup_parts(data).await?;
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
        let relation_writes = original.prepare_relation_writes(record.clone()).await?;
        ensure_relation_tables(&relation_writes).await?;
        let mut stmt =
            RawSqlStmt::new("BEGIN TRANSACTION; UPSERT ONLY $record CONTENT $data RETURN AFTER;");
        stmt = stmt
            .bind("record", record.clone())
            .bind("data", content.clone());
        let (stmt_with_relations, _) = append_relation_sync_to_stmt(stmt, &relation_writes, "rel")?;
        let mut stmt = stmt_with_relations;
        stmt.sql.push_str("COMMIT TRANSACTION;");
        let mut result = query_bound_checked(stmt).await?;
        let row: Option<SurrealDbValue> = result.take(1)?;
        let row = row.ok_or(DBError::EmptyResult("save"))?;
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
    async fn list_all() -> Result<Vec<T>> {
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

    /// Starts a full-table list query that can be ordered before awaiting.
    pub fn list() -> ListQuery<T> {
        ListQuery::new()
    }

    /// Lists every row ordered by `id` or the declared `#[pagin]` field.
    pub async fn list_ordered(field: &str, order: Order) -> Result<Vec<T>>
    where
        T: PaginationMeta,
    {
        let field = Self::validate_list_order_field(field)?;
        let db = get_db()?;
        let mut result = db
            .query(QueryKind::all_by_order(T::storage_table(), order, field))
            .bind(("table", Table::from(T::storage_table())))
            .await?
            .check()?;

        if T::has_foreign_fields() || T::has_relation_fields() {
            let rows: Vec<SurrealDbValue> = result.take(1)?;
            return raw_rows_to_public_hydrated::<T>(rows).await;
        }

        let rows: Vec<T::Stored> = result.take(1)?;
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

    async fn pagin_with_order(
        count: i64,
        cursor: Option<PageCursor>,
        order: Order,
    ) -> Result<Page<T>>
    where
        T: PaginationMeta,
        T::Stored: serde::de::DeserializeOwned,
    {
        let field = T::pagination_field().ok_or_else(|| {
            DBError::InvalidModel(format!(
                "model `{}` does not declare a #[pagin] field",
                std::any::type_name::<T>()
            ))
        })?;
        let plan = PaginationPlan::new(field, order);

        let requested = usize::try_from(count).map_err(|_| {
            DBError::InvalidModel(format!("pagination count must be positive, got `{count}`"))
        })?;
        if requested == 0 {
            return Err(
                DBError::InvalidModel("pagination count must be positive".to_owned()).into(),
            );
        }

        let query_count = count.checked_add(1).ok_or_else(|| {
            DBError::InvalidModel(format!(
                "pagination count `{count}` overflowed the lookahead window"
            ))
        })?;

        let stmt = plan.build_stmt(T::storage_table(), query_count, cursor.as_ref())?;
        let mut rows = crate::query::query_bound_take::<serde_json::Value>(stmt, Some(1)).await?;
        let next = if rows.len() > requested {
            rows.truncate(requested);
            Some(
                plan.build_cursor(
                    rows.last()
                        .expect("truncated page should retain its last row"),
                )?,
            )
        } else {
            None
        };

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(decode_hydrated_row::<T>(row).await?);
        }

        Ok(Page { items, next })
    }

    /// Lists one descending keyset page using the model's `#[pagin]` field.
    pub async fn pagin_desc(count: i64, cursor: Option<PageCursor>) -> Result<Page<T>>
    where
        T: PaginationMeta,
        T::Stored: serde::de::DeserializeOwned,
    {
        Self::pagin_with_order(count, cursor, Order::Desc).await
    }

    /// Lists one ascending keyset page using the model's `#[pagin]` field.
    pub async fn pagin_asc(count: i64, cursor: Option<PageCursor>) -> Result<Page<T>>
    where
        T: PaginationMeta,
        T::Stored: serde::de::DeserializeOwned,
    {
        Self::pagin_with_order(count, cursor, Order::Asc).await
    }

    /// Batch-upserts models by their `id` field and returns normalized rows.
    /// Saves many rows in chunks and returns normalized results.
    pub async fn save_many(data: Vec<T>) -> Result<Vec<T>> {
        if data.is_empty() {
            return Ok(vec![]);
        }

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

            ensure_relation_tables(&relation_writes).await?;
            let mut stmt = RawSqlStmt::new(sql);
            for (idx, (record, content, _)) in prepared.iter().enumerate() {
                stmt = stmt
                    .bind(format!("record_{idx}"), record.clone())
                    .bind(format!("data_{idx}"), content.clone());
            }
            let (stmt_with_relations, _) =
                append_relation_sync_to_stmt(stmt, &relation_writes, "rel")?;
            let mut stmt = stmt_with_relations;
            stmt.sql.push_str("COMMIT TRANSACTION;");

            let mut result = query_bound_checked(stmt).await?;

            for (idx, (_, _, id)) in prepared.clone().into_iter().enumerate() {
                let row: Option<SurrealDbValue> = result.take(idx + 1)?;
                let row = row.ok_or(DBError::EmptyResult("save_many"))?;
                let stored = decode_saved_row_from_model::<T>(row, id, &originals[idx])?;
                match T::hydrate_foreign(stored).await {
                    Ok(value) => inserted_all.push(value),
                    Err(err) => {
                        let db = get_db()?;
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
