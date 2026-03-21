use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue, Table};

use crate::connection::get_db;
use crate::model::meta::ResolveRecordId;
use crate::model::relation::RelationMeta;
use crate::query::builder::QueryKind;

/// Edge payload used with relation-table inserts.
#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub struct RelationEdge {
    /// Source record id.
    #[serde(rename = "in")]
    pub _in: RecordId,
    /// Target record id.
    pub out: RecordId,
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
}

impl<T> GraphCrud for T where T: ResolveRecordId + Send + Sync {}

/// Free-function wrapper for [`GraphRepo::relate_at`].
pub async fn relate_at(in_id: RecordId, out_id: RecordId, rel: &str) -> Result<()> {
    GraphRepo::relate_at(in_id, out_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::unrelate_at`].
pub async fn unrelate_at(self_id: RecordId, target_id: RecordId, rel: &str) -> Result<()> {
    GraphRepo::unrelate_at(self_id, target_id, rel).await
}

/// Free-function wrapper for [`GraphRepo::out_ids`].
pub async fn out_ids(in_id: RecordId, rel: &str, out_table: &str) -> Result<Vec<RecordId>> {
    GraphRepo::out_ids(in_id, rel, out_table).await
}

/// Free-function wrapper for [`GraphRepo::in_ids`].
pub async fn in_ids(out_id: RecordId, rel: &str, in_table: &str) -> Result<Vec<RecordId>> {
    GraphRepo::in_ids(out_id, rel, in_table).await
}
