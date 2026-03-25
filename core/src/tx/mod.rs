use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use surrealdb::IndexedResults;
use surrealdb::opt;
use surrealdb::types::{SurrealValue, Value};

use crate::connection::get_db;

/// One SQL statement plus bind values for a transaction run.
pub struct TxStmt {
    /// SurrealQL text to execute.
    pub sql: String,
    /// Named bind values applied before execution.
    pub bindings: BTreeMap<String, Value>,
}

impl TxStmt {
    /// Creates a transaction statement from raw SQL.
    pub fn new<S: Into<String>>(sql: S) -> Self {
        Self {
            sql: sql.into(),
            bindings: BTreeMap::new(),
        }
    }

    /// Adds one bind value and returns the updated statement.
    pub fn bind<K: Into<String>, V: SurrealValue>(mut self, key: K, val: V) -> Self {
        self.bindings.insert(key.into(), val.into_value());
        self
    }
}

/// Runs multiple statements inside one explicit SurrealDB transaction.
pub struct TxRunner;

/// Collected results for each statement in a transaction.
pub struct TxResults {
    statements: Vec<IndexedResults>,
}

impl TxResults {
    /// Returns the number of statement result sets.
    pub fn len(&self) -> usize {
        self.statements.len()
    }

    /// Returns `true` when no statement results are stored.
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// Returns an immutable result set by statement index.
    pub fn get(&self, index: usize) -> Option<&IndexedResults> {
        self.statements.get(index)
    }

    /// Returns a mutable result set by statement index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut IndexedResults> {
        self.statements.get_mut(index)
    }

    /// Decodes one statement result entry from the transaction output.
    pub fn take<R>(
        &mut self,
        statement_index: usize,
        result_index: impl opt::QueryResult<R>,
    ) -> Result<R>
    where
        R: SurrealValue,
    {
        let results = self.statements.get_mut(statement_index).ok_or_else(|| {
            anyhow!("transaction statement index out of range: {statement_index}")
        })?;
        Ok(results.take(result_index)?)
    }

    /// Consumes the wrapper and returns the raw SurrealDB result sets.
    pub fn into_inner(self) -> Vec<IndexedResults> {
        self.statements
    }
}

impl TxRunner {
    /// Executes all statements in a single transaction and returns per-statement results.
    pub async fn run(stmts: Vec<TxStmt>) -> Result<TxResults> {
        let db = get_db()?;
        let tx = db.as_ref().clone().begin().await?;
        let mut responses = Vec::with_capacity(stmts.len());

        for stmt in stmts {
            let sql_for_error = stmt.sql.clone();
            let mut query = tx.query(&stmt.sql);
            for (k, v) in stmt.bindings {
                query = query.bind((k, v));
            }
            let response = query
                .await
                .map_err(|e| anyhow!("tx query failed: `{sql_for_error}`: {e}"))?
                .check()
                .map_err(|e| anyhow!("tx response check failed: `{sql_for_error}`: {e}"))?;
            responses.push(response);
        }

        tx.commit().await?;

        if responses.is_empty() {
            let response = db.query("RETURN NONE;").await?.check()?;
            responses.push(response);
        }

        Ok(TxResults {
            statements: responses,
        })
    }
}

/// Free-function wrapper for [`TxRunner::run`].
pub async fn run_tx(stmts: Vec<TxStmt>) -> Result<TxResults> {
    TxRunner::run(stmts).await
}
