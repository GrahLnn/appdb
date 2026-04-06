use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue, Table, Value as SurrealDbValue};

use crate::connection::get_db;
use crate::model::meta::{ModelMeta, ResolveRecordId};
use crate::model::relation::{RelationMeta, ensure_relation_name};
use crate::query::builder::QueryKind;
use crate::{ForeignModel, StoredModel};

/// Edge payload used with relation-table inserts.
#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub struct RelationEdge {
    /// Source record id.
    #[serde(rename = "in")]
    pub _in: RecordId,
    /// Target record id.
    pub out: RecordId,
}

/// Ordered edge payload used by `#[relate(...)]` field synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue)]
pub struct OrderedRelationEdge {
    /// Source record id.
    #[serde(rename = "in")]
    pub _in: Option<RecordId>,
    /// Target record id.
    pub out: RecordId,
    /// Stable position for vector-shaped relation fields.
    pub position: i64,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct OrderedRelationEdgeRow {
    source: RecordId,
    out: RecordId,
    position: i64,
}

impl From<OrderedRelationEdgeRow> for OrderedRelationEdge {
    fn from(value: OrderedRelationEdgeRow) -> Self {
        Self {
            _in: Some(value.source),
            out: value.out,
            position: value.position,
        }
    }
}

/// Repository-style helpers for SurrealDB relation tables.
pub struct GraphRepo;

impl GraphRepo {
    /// Creates a relation row from `in_id` to `out_id` in `rel`.
    pub async fn relate_at(in_id: RecordId, out_id: RecordId, rel: &str) -> Result<()> {
        let db = get_db()?;
        let sql = QueryKind::relate(&in_id, &out_id, rel);
        db.query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .bind(("out", out_id))
            .await?
            .check()?;
        Ok(())
    }

    /// Creates a relation row from `target_id` back to `self_id` in `rel`.
    pub async fn back_relate_at(self_id: RecordId, target_id: RecordId, rel: &str) -> Result<()> {
        Self::relate_at(target_id, self_id, rel).await
    }

    /// Deletes a single outgoing relation from `self_id` to `target_id`.
    pub async fn unrelate_at(self_id: RecordId, target_id: RecordId, rel: &str) -> Result<()> {
        let db = get_db()?;
        db.query(QueryKind::unrelate(&self_id, &target_id, rel))
            .bind(("rel", Table::from(rel)))
            .bind(("in", self_id))
            .bind(("out", target_id))
            .await?
            .check()?;
        Ok(())
    }

    /// Deletes all outgoing relations for `self_id` in `rel`.
    pub async fn unrelate_all(self_id: RecordId, rel: &str) -> Result<()> {
        let db = get_db()?;
        db.query(QueryKind::unrelate_all(&self_id, rel))
            .bind(("rel", Table::from(rel)))
            .bind(("in", self_id))
            .await?
            .check()?;
        Ok(())
    }

    /// Lists target record ids reachable from `in_id` through `rel`.
    pub async fn out_ids(in_id: RecordId, rel: &str, out_table: &str) -> Result<Vec<RecordId>> {
        let sql = QueryKind::select_out_ids(&in_id, rel, out_table);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .bind(("out_table", out_table.to_owned()))
            .await?
            .check()?;
        let rows: Vec<RecordId> = result.take(0)?;
        Ok(rows)
    }

    /// Lists all outgoing target record ids for `in_id` through `rel`.
    pub async fn outgoing_ids(in_id: RecordId, rel: &str) -> Result<Vec<RecordId>> {
        let sql = QueryKind::select_all_out_ids(&in_id, rel);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .await?
            .check()?;
        let rows: Vec<RecordId> = result.take(0)?;
        Ok(rows)
    }

    /// Loads outgoing related records from `in_id` through `rel` for one target model table.
    pub async fn outgoing<T>(in_id: RecordId, rel: &str) -> Result<Vec<T>>
    where
        T: ModelMeta + StoredModel + ForeignModel,
        T::Stored: serde::de::DeserializeOwned,
    {
        let sql = QueryKind::select_outgoing_rows(&in_id, rel, T::storage_table());
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .bind(("out_table", T::storage_table().to_owned()))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(1)?;
        crate::repository::raw_rows_to_public_hydrated::<T>(rows).await
    }

    /// Counts all outgoing edges for `in_id` through `rel`.
    pub async fn outgoing_count(in_id: RecordId, rel: &str) -> Result<i64> {
        let sql = QueryKind::count_all_outgoing(&in_id, rel);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .await?
            .check()?;
        let count: Option<i64> = result.take(0)?;
        Ok(count.unwrap_or(0))
    }

    /// Counts outgoing edges for `in_id` through `rel` filtered by one target model table.
    pub async fn outgoing_count_as<T>(in_id: RecordId, rel: &str) -> Result<i64>
    where
        T: ModelMeta + StoredModel + ForeignModel,
    {
        let sql = QueryKind::count_outgoing_in_table(&in_id, rel, T::storage_table());
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .bind(("out_table", T::storage_table().to_owned()))
            .await?
            .check()?;
        let count: Option<i64> = result.take(0)?;
        Ok(count.unwrap_or(0))
    }

    /// Lists ordered outgoing relation edges for `in_id` through `rel`.
    pub async fn out_edges(in_id: RecordId, rel: &str) -> Result<Vec<OrderedRelationEdge>> {
        let sql = QueryKind::select_out_edges(&in_id, rel);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("in", in_id))
            .await?
            .check()?;
        let rows: Vec<OrderedRelationEdgeRow> = result.take(0)?;
        Ok(rows.into_iter().map(OrderedRelationEdge::from).collect())
    }

    /// Lists source record ids that point to `out_id` through `rel`.
    pub async fn in_ids(out_id: RecordId, rel: &str, in_table: &str) -> Result<Vec<RecordId>> {
        let sql = QueryKind::select_in_ids(&out_id, rel, in_table);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("out", out_id))
            .bind(("in_table", in_table.to_owned()))
            .await?
            .check()?;
        let rows: Vec<RecordId> = result.take(0)?;
        Ok(rows)
    }

    /// Lists ordered incoming relation edges for `out_id` through `rel`.
    pub async fn in_edges(out_id: RecordId, rel: &str) -> Result<Vec<OrderedRelationEdge>> {
        let sql = QueryKind::select_in_edges(&out_id, rel);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("out", out_id))
            .await?
            .check()?;
        let rows: Vec<OrderedRelationEdgeRow> = result.take(0)?;
        Ok(rows.into_iter().map(OrderedRelationEdge::from).collect())
    }

    /// Lists all incoming source record ids for `out_id` through `rel`.
    pub async fn incoming_ids(out_id: RecordId, rel: &str) -> Result<Vec<RecordId>> {
        let sql = QueryKind::select_all_in_ids(&out_id, rel);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("out", out_id))
            .await?
            .check()?;
        let rows: Vec<RecordId> = result.take(0)?;
        Ok(rows)
    }

    /// Loads incoming related records into one source model table through `rel`.
    pub async fn incoming<T>(out_id: RecordId, rel: &str) -> Result<Vec<T>>
    where
        T: ModelMeta + StoredModel + ForeignModel,
        T::Stored: serde::de::DeserializeOwned,
    {
        let sql = QueryKind::select_incoming_rows(&out_id, rel, T::storage_table());
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("out", out_id))
            .bind(("in_table", T::storage_table().to_owned()))
            .await?
            .check()?;
        let rows: Vec<SurrealDbValue> = result.take(1)?;
        crate::repository::raw_rows_to_public_hydrated::<T>(rows).await
    }

    /// Counts all incoming edges for `out_id` through `rel`.
    pub async fn incoming_count(out_id: RecordId, rel: &str) -> Result<i64> {
        let sql = QueryKind::count_all_incoming(&out_id, rel);
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("out", out_id))
            .await?
            .check()?;
        let count: Option<i64> = result.take(0)?;
        Ok(count.unwrap_or(0))
    }

    /// Counts incoming edges for `out_id` through `rel` filtered by one source model table.
    pub async fn incoming_count_as<T>(out_id: RecordId, rel: &str) -> Result<i64>
    where
        T: ModelMeta + StoredModel + ForeignModel,
    {
        let sql = QueryKind::count_incoming_in_table(&out_id, rel, T::storage_table());
        let db = get_db()?;
        let mut result = db
            .query(sql)
            .bind(("rel", Table::from(rel)))
            .bind(("out", out_id))
            .bind(("in_table", T::storage_table().to_owned()))
            .await?
            .check()?;
        let count: Option<i64> = result.take(0)?;
        Ok(count.unwrap_or(0))
    }

    /// Inserts multiple relation rows into the given relation table.
    pub async fn insert_relation(rel: &str, data: Vec<RelationEdge>) -> Result<Vec<RelationEdge>> {
        let db = get_db()?;
        let relate: Vec<RelationEdge> = db.insert(rel).relation(data).await?;
        Ok(relate)
    }
}

/// Convenience graph methods for values that can resolve to one record id.
#[async_trait]
pub trait GraphCrud: ResolveRecordId + Send + Sync {
    /// Creates a relation from `self` to `target`.
    async fn relate<R, T>(&self, target: &T) -> Result<()>
    where
        R: RelationMeta + Send + Sync,
        T: ResolveRecordId + Send + Sync,
    {
        GraphRepo::relate_at(
            self.resolve_record_id().await?,
            target.resolve_record_id().await?,
            R::relation_name(),
        )
        .await
    }

    /// Creates a relation from `self` to `target` using a raw relation-table name.
    async fn relate_by_name<T>(&self, target: &T, relation: &str) -> Result<()>
    where
        T: ResolveRecordId + Send + Sync,
    {
        ensure_relation_name(relation)?;
        GraphRepo::relate_at(
            self.resolve_record_id().await?,
            target.resolve_record_id().await?,
            relation,
        )
        .await
    }

    /// Creates a relation from `target` back to `self`.
    async fn back_relate<R, T>(&self, target: &T) -> Result<()>
    where
        R: RelationMeta + Send + Sync,
        T: ResolveRecordId + Send + Sync,
    {
        GraphRepo::back_relate_at(
            self.resolve_record_id().await?,
            target.resolve_record_id().await?,
            R::relation_name(),
        )
        .await
    }

    /// Creates a relation from `target` back to `self` using a raw relation-table name.
    async fn back_relate_by_name<T>(&self, target: &T, relation: &str) -> Result<()>
    where
        T: ResolveRecordId + Send + Sync,
    {
        ensure_relation_name(relation)?;
        GraphRepo::back_relate_at(
            self.resolve_record_id().await?,
            target.resolve_record_id().await?,
            relation,
        )
        .await
    }

    /// Deletes a relation from `self` to `target`.
    async fn unrelate<R, T>(&self, target: &T) -> Result<()>
    where
        R: RelationMeta + Send + Sync,
        T: ResolveRecordId + Send + Sync,
    {
        GraphRepo::unrelate_at(
            self.resolve_record_id().await?,
            target.resolve_record_id().await?,
            R::relation_name(),
        )
        .await
    }

    /// Deletes a relation from `self` to `target` using a raw relation-table name.
    async fn unrelate_by_name<T>(&self, target: &T, relation: &str) -> Result<()>
    where
        T: ResolveRecordId + Send + Sync,
    {
        ensure_relation_name(relation)?;
        GraphRepo::unrelate_at(
            self.resolve_record_id().await?,
            target.resolve_record_id().await?,
            relation,
        )
        .await
    }
}

impl<T> GraphCrud for T where T: ResolveRecordId + Send + Sync {}

/// Free-function wrapper for [`GraphRepo::relate_at`].
pub async fn relate_at(in_id: RecordId, out_id: RecordId, rel: &str) -> Result<()> {
    GraphRepo::relate_at(in_id, out_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::back_relate_at`].
pub async fn back_relate_at(self_id: RecordId, target_id: RecordId, rel: &str) -> Result<()> {
    GraphRepo::back_relate_at(self_id, target_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::unrelate_at`].
pub async fn unrelate_at(self_id: RecordId, target_id: RecordId, rel: &str) -> Result<()> {
    GraphRepo::unrelate_at(self_id, target_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::out_ids`].
pub async fn out_ids(in_id: RecordId, rel: &str, out_table: &str) -> Result<Vec<RecordId>> {
    GraphRepo::out_ids(in_id, rel, out_table).await
}

/// Free-function wrapper for [`GraphRepo::outgoing_ids`].
pub async fn outgoing_ids(in_id: RecordId, rel: &str) -> Result<Vec<RecordId>> {
    GraphRepo::outgoing_ids(in_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::outgoing`].
pub async fn outgoing<T>(in_id: RecordId, rel: &str) -> Result<Vec<T>>
where
    T: ModelMeta + StoredModel + ForeignModel,
    T::Stored: serde::de::DeserializeOwned,
{
    GraphRepo::outgoing::<T>(in_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::outgoing_count`].
pub async fn outgoing_count(in_id: RecordId, rel: &str) -> Result<i64> {
    GraphRepo::outgoing_count(in_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::outgoing_count_as`].
pub async fn outgoing_count_as<T>(in_id: RecordId, rel: &str) -> Result<i64>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    GraphRepo::outgoing_count_as::<T>(in_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::out_edges`].
pub async fn out_edges(in_id: RecordId, rel: &str) -> Result<Vec<OrderedRelationEdge>> {
    GraphRepo::out_edges(in_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::in_ids`].
pub async fn in_ids(out_id: RecordId, rel: &str, in_table: &str) -> Result<Vec<RecordId>> {
    GraphRepo::in_ids(out_id, rel, in_table).await
}

/// Free-function wrapper for [`GraphRepo::in_edges`].
pub async fn in_edges(out_id: RecordId, rel: &str) -> Result<Vec<OrderedRelationEdge>> {
    GraphRepo::in_edges(out_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::incoming_ids`].
pub async fn incoming_ids(out_id: RecordId, rel: &str) -> Result<Vec<RecordId>> {
    GraphRepo::incoming_ids(out_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::incoming`].
pub async fn incoming<T>(out_id: RecordId, rel: &str) -> Result<Vec<T>>
where
    T: ModelMeta + StoredModel + ForeignModel,
    T::Stored: serde::de::DeserializeOwned,
{
    GraphRepo::incoming::<T>(out_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::incoming_count`].
pub async fn incoming_count(out_id: RecordId, rel: &str) -> Result<i64> {
    GraphRepo::incoming_count(out_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::incoming_count_as`].
pub async fn incoming_count_as<T>(out_id: RecordId, rel: &str) -> Result<i64>
where
    T: ModelMeta + StoredModel + ForeignModel,
{
    GraphRepo::incoming_count_as::<T>(out_id, rel).await
}
