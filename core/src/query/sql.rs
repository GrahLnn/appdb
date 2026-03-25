use std::collections::BTreeMap;

use anyhow::Result;
use surrealdb::IndexedResults;
use surrealdb::types::{SurrealValue, Value};

use crate::connection::get_db;
use crate::error::DBError;

#[derive(Debug, Clone, Default)]
/// Raw SQL statement plus named bind values.
pub struct RawSqlStmt {
    /// SurrealQL text to execute.
    pub sql: String,
    /// Named bind values used by the statement.
    pub bindings: BTreeMap<String, Value>,
}

impl RawSqlStmt {
    /// Creates a new raw statement.
    pub fn new<S: Into<String>>(sql: S) -> Self {
        Self {
            sql: sql.into(),
            bindings: BTreeMap::new(),
        }
    }

    /// Adds a bind value and returns the updated statement.
    pub fn bind<K: Into<String>, V: SurrealValue>(mut self, key: K, value: V) -> Self {
        self.bindings.insert(key.into(), value.into_value());
        self
    }
}

/// Raw-query helper for advanced SurrealQL usage.
pub struct RawSql;

impl RawSql {
    /// Executes raw SQL without calling `check()` on the response.
    pub async fn query_unchecked(sql: &str) -> Result<IndexedResults> {
        let db = get_db()?;
        let result = db.query(sql).await?;
        Ok(result)
    }

    /// Executes raw SQL and validates the response with `check()`.
    pub async fn query_checked(sql: &str) -> Result<IndexedResults> {
        let result = Self::query_unchecked(sql).await?;
        result
            .check()
            .map_err(|err| DBError::QueryResponse(err.to_string()).into())
    }

    /// Executes raw SQL and decodes one result set into typed values.
    pub async fn query_take_typed<T>(sql: &str, idx: Option<usize>) -> Result<Vec<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_checked(sql).await?;
        let records: Vec<T> = result.take(idx.unwrap_or(0))?;
        Ok(records)
    }

    /// Executes raw SQL and decodes the first result as an optional typed value.
    pub async fn query_return_typed<T>(sql: &str) -> Result<Option<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_checked(sql).await?;
        let value: Option<T> = result.take(0)?;
        Ok(value)
    }

    /// Executes a bound raw statement without calling `check()`.
    pub async fn query_stmt_unchecked(stmt: RawSqlStmt) -> Result<IndexedResults> {
        let db = get_db()?;
        let mut query = db.query(&stmt.sql);
        for (key, value) in stmt.bindings {
            query = query.bind((key, value));
        }
        let result = query.await?;
        Ok(result)
    }

    /// Executes a bound raw statement and validates the response.
    pub async fn query_stmt_checked(stmt: RawSqlStmt) -> Result<IndexedResults> {
        let result = Self::query_stmt_unchecked(stmt).await?;
        result
            .check()
            .map_err(|err| DBError::QueryResponse(err.to_string()).into())
    }

    /// Executes a bound statement and decodes one result set into typed values.
    pub async fn query_stmt_take_typed<T>(stmt: RawSqlStmt, idx: Option<usize>) -> Result<Vec<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_stmt_checked(stmt).await?;
        let records: Vec<T> = result.take(idx.unwrap_or(0))?;
        Ok(records)
    }

    /// Executes a bound statement and decodes the first result as an optional typed value.
    pub async fn query_stmt_return_typed<T>(stmt: RawSqlStmt) -> Result<Option<T>>
    where
        T: SurrealValue + 'static,
    {
        let mut result = Self::query_stmt_checked(stmt).await?;
        let value: Option<T> = result.take(0)?;
        Ok(value)
    }
}

/// Free-function wrapper for [`RawSql::query_unchecked`].
pub async fn query_raw(sql: &str) -> Result<IndexedResults> {
    RawSql::query_unchecked(sql).await
}

/// Free-function wrapper for [`RawSql::query_checked`].
pub async fn query_checked(sql: &str) -> Result<IndexedResults> {
    RawSql::query_checked(sql).await
}

/// Free-function wrapper for [`RawSql::query_take_typed`].
pub async fn query_take<T>(sql: &str, idx: Option<usize>) -> Result<Vec<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_take_typed(sql, idx).await
}

/// Free-function wrapper for [`RawSql::query_return_typed`].
pub async fn query_return<T>(sql: &str) -> Result<Option<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_return_typed(sql).await
}

/// Free-function wrapper for [`RawSql::query_stmt_unchecked`].
pub async fn query_bound(stmt: RawSqlStmt) -> Result<IndexedResults> {
    RawSql::query_stmt_unchecked(stmt).await
}

/// Free-function wrapper for [`RawSql::query_stmt_checked`].
pub async fn query_bound_checked(stmt: RawSqlStmt) -> Result<IndexedResults> {
    RawSql::query_stmt_checked(stmt).await
}

/// Free-function wrapper for [`RawSql::query_stmt_take_typed`].
pub async fn query_bound_take<T>(stmt: RawSqlStmt, idx: Option<usize>) -> Result<Vec<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_stmt_take_typed(stmt, idx).await
}

/// Free-function wrapper for [`RawSql::query_stmt_return_typed`].
pub async fn query_bound_return<T>(stmt: RawSqlStmt) -> Result<Option<T>>
where
    T: SurrealValue + 'static,
{
    RawSql::query_stmt_return_typed(stmt).await
}
