use std::collections::BTreeMap;

use anyhow::Result;
use surrealdb::types::{SurrealValue, Value};
use surrealdb::IndexedResults;

use crate::connection::get_db;
use crate::error::DBError;

#[derive(Debug, Clone, Default)]
pub struct RawSqlStmt {
    pub sql: String,
    pub bindings: BTreeMap<String, Value>,
}

impl RawSqlStmt {
    pub fn new<S: Into<String>>(sql: S) -> Self {
        Self {
            sql: sql.into(),
            bindings: BTreeMap::new(),
        }
    }

    pub fn bind<K: Into<String>, V: SurrealValue>(mut self, key: K, value: V) -> Self {
        self.bindings.insert(key.into(), value.into_value());
        self
    }
}

pub struct RawSql;

impl RawSql {
    pub async fn query_unchecked(sql: &str) -> Result<IndexedResults> {
        let db = get_db()?;
        let result = db.query(sql).await?;
        Ok(result)
    }

    pub async fn query_checked(sql: &str) -> Result<IndexedResults> {
        let result = Self::query_unchecked(sql).await?;
        result
            .check()
            .map_err(|err| DBError::QueryResponse(err.to_string()).into())
    }

    pub async fn query_take_typed<T>(sql: &str, idx: Option<usize>) -> Result<Vec<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_checked(sql).await?;
        let records: Vec<T> = result.take(idx.unwrap_or(0))?;
        Ok(records)
    }

    pub async fn query_return_typed<T>(sql: &str) -> Result<Option<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_checked(sql).await?;
        let value: Option<T> = result.take(0)?;
        Ok(value)
    }

    pub async fn query_stmt_unchecked(stmt: RawSqlStmt) -> Result<IndexedResults> {
        let db = get_db()?;
        let mut query = db.query(&stmt.sql);
        for (key, value) in stmt.bindings {
            query = query.bind((key, value));
        }
        let result = query.await?;
        Ok(result)
    }

    pub async fn query_stmt_checked(stmt: RawSqlStmt) -> Result<IndexedResults> {
        let result = Self::query_stmt_unchecked(stmt).await?;
        result
            .check()
            .map_err(|err| DBError::QueryResponse(err.to_string()).into())
    }

    pub async fn query_stmt_take_typed<T>(stmt: RawSqlStmt, idx: Option<usize>) -> Result<Vec<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_stmt_checked(stmt).await?;
        let records: Vec<T> = result.take(idx.unwrap_or(0))?;
        Ok(records)
    }

    pub async fn query_stmt_return_typed<T>(stmt: RawSqlStmt) -> Result<Option<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_stmt_checked(stmt).await?;
        let value: Option<T> = result.take(0)?;
        Ok(value)
    }
}

pub async fn query_raw(sql: &str) -> Result<IndexedResults> {
    RawSql::query_unchecked(sql).await
}

pub async fn query_checked(sql: &str) -> Result<IndexedResults> {
    RawSql::query_checked(sql).await
}

pub async fn query_take<T>(sql: &str, idx: Option<usize>) -> Result<Vec<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_take_typed(sql, idx).await
}

pub async fn query_return<T>(sql: &str) -> Result<Option<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_return_typed(sql).await
}

pub async fn query_bound(stmt: RawSqlStmt) -> Result<IndexedResults> {
    RawSql::query_stmt_unchecked(stmt).await
}

pub async fn query_bound_checked(stmt: RawSqlStmt) -> Result<IndexedResults> {
    RawSql::query_stmt_checked(stmt).await
}

pub async fn query_bound_take<T>(stmt: RawSqlStmt, idx: Option<usize>) -> Result<Vec<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_stmt_take_typed(stmt, idx).await
}

pub async fn query_bound_return<T>(stmt: RawSqlStmt) -> Result<Option<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_stmt_return_typed(stmt).await
}
