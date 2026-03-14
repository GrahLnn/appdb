use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use surrealdb::opt;
use surrealdb::types::{SurrealValue, Value};
use surrealdb::IndexedResults;

use crate::connection::get_db;

pub struct TxStmt {
    pub sql: String,
    pub bindings: BTreeMap<String, Value>,
}

impl TxStmt {
    pub fn new<S: Into<String>>(sql: S) -> Self {
        Self {
            sql: sql.into(),
            bindings: BTreeMap::new(),
        }
    }

    pub fn bind<K: Into<String>, V: SurrealValue>(mut self, key: K, val: V) -> Self {
        self.bindings.insert(key.into(), val.into_value());
        self
    }
}

pub struct TxRunner;

pub struct TxResults {
    statements: Vec<IndexedResults>,
}

impl TxResults {
    pub fn len(&self) -> usize {
        self.statements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&IndexedResults> {
        self.statements.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut IndexedResults> {
        self.statements.get_mut(index)
    }

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

    pub fn into_inner(self) -> Vec<IndexedResults> {
        self.statements
    }
}

impl TxRunner {
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

pub async fn run_tx(stmts: Vec<TxStmt>) -> Result<TxResults> {
    TxRunner::run(stmts).await
}
