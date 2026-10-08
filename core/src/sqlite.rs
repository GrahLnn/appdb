//! SQLite storage for plain appdb models.
//!
//! This backend keeps the model/serialization boundary independent from
//! SurrealDB. A model supplies its table, stable key, and optional keyset
//! ordering value; serde JSON is the default payload codec and can be replaced
//! for encrypted or otherwise specialized storage. The table is a `WITHOUT
//! ROWID` table with one primary-key BLOB and one optional ordering BLOB, so a
//! write updates one row instead of appending an update fragment.

use crate::error::{DBError, classify_db_error_text};
use crate::serde_utils::id::Id;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Result type used by the SQLite adapter.
pub type SqliteResult<T> = Result<T, DBError>;

/// A stable root key accepted by the appdb model layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SqliteKey {
    /// Text key.
    Text(String),
    /// Signed 64-bit integer key.
    Integer(i64),
}

impl From<&Id> for SqliteKey {
    fn from(value: &Id) -> Self {
        match value {
            Id::String(value) => Self::Text(value.clone()),
            Id::Number(value) => Self::Integer(*value),
        }
    }
}

impl From<String> for SqliteKey {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for SqliteKey {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<i64> for SqliteKey {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl SqliteKey {
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Text(value) => prefixed_bytes(0x01, value.as_bytes()),
            Self::Integer(value) => prefixed_bytes(0x02, &ordered_i64(*value)),
        }
    }
}

/// A scalar used for an indexed keyset pagination field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SqliteOrderKey {
    /// UTF-8 text ordering.
    Text(String),
    /// Signed 64-bit integer ordering.
    Integer(i64),
    /// Finite floating-point ordering.
    Real(f64),
    /// Boolean ordering (`false` before `true`).
    Boolean(bool),
}

impl SqliteOrderKey {
    /// Creates a finite floating-point ordering key.
    pub fn real(value: f64) -> SqliteResult<Self> {
        if value.is_finite() {
            Ok(Self::Real(value))
        } else {
            Err(DBError::InvalidModel(
                "SQLite ordering keys must be finite".to_owned(),
            ))
        }
    }

    fn to_bytes(&self) -> SqliteResult<Vec<u8>> {
        match self {
            Self::Text(value) => Ok(prefixed_bytes(0x10, value.as_bytes())),
            Self::Integer(value) => Ok(prefixed_bytes(0x11, &ordered_i64(*value))),
            Self::Real(value) => {
                if !value.is_finite() {
                    return Err(DBError::InvalidModel(
                        "SQLite ordering keys must be finite".to_owned(),
                    ));
                }
                Ok(prefixed_bytes(0x12, &ordered_f64(*value)))
            }
            Self::Boolean(value) => Ok(prefixed_bytes(0x13, &[u8::from(*value)])),
        }
    }
}

fn prefixed_bytes(prefix: u8, bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len() + 1);
    output.push(prefix);
    output.extend_from_slice(bytes);
    output
}

fn ordered_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

fn ordered_f64(value: f64) -> [u8; 8] {
    let bits = value.to_bits();
    let ordered = if bits & (1_u64 << 63) != 0 {
        !bits
    } else {
        bits ^ (1_u64 << 63)
    };
    ordered.to_be_bytes()
}

fn quote_identifier(identifier: &str) -> SqliteResult<String> {
    let mut chars = identifier.chars();
    let Some(first) = chars.next() else {
        return Err(DBError::InvalidIdentifier(
            "SQLite table name cannot be empty".to_owned(),
        ));
    };
    if identifier.to_ascii_lowercase().starts_with("sqlite_")
        || !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(DBError::InvalidIdentifier(format!(
            "invalid SQLite table name `{identifier}`"
        )));
    }
    Ok(format!("\"{identifier}\""))
}

type IdFn<T> = dyn Fn(&T) -> SqliteKey + Send + Sync;
type OrderFn<T> = dyn Fn(&T) -> SqliteResult<SqliteOrderKey> + Send + Sync;
type EncodeFn<T> = dyn Fn(&T) -> SqliteResult<Vec<u8>> + Send + Sync;
type DecodeFn<T> = dyn Fn(&[u8]) -> SqliteResult<T> + Send + Sync;

/// Backend-neutral description of one serializable SQLite model.
pub struct SqliteModel<T> {
    table: String,
    id: Arc<IdFn<T>>,
    order: Option<Arc<OrderFn<T>>>,
    encode: Arc<EncodeFn<T>>,
    decode: Arc<DecodeFn<T>>,
}

impl<T> SqliteModel<T> {
    /// Builds a model using serde JSON for the payload.
    pub fn json(
        table: impl Into<String>,
        id: impl Fn(&T) -> SqliteKey + Send + Sync + 'static,
    ) -> SqliteResult<Self>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
    {
        let table = table.into();
        quote_identifier(&table)?;
        Ok(Self {
            table,
            id: Arc::new(id),
            order: None,
            encode: Arc::new(|value| {
                serde_json::to_vec(value)
                    .map_err(|error| DBError::Decode(format!("SQLite JSON encode failed: {error}")))
            }),
            decode: Arc::new(|payload| {
                serde_json::from_slice(payload)
                    .map_err(|error| DBError::Decode(format!("SQLite JSON decode failed: {error}")))
            }),
        })
    }

    /// Adds a required keyset ordering value to the model.
    pub fn with_order(
        mut self,
        order: impl Fn(&T) -> SqliteResult<SqliteOrderKey> + Send + Sync + 'static,
    ) -> Self {
        self.order = Some(Arc::new(order));
        self
    }

    /// Replaces the default JSON payload codec.
    pub fn with_codec(
        mut self,
        encode: impl Fn(&T) -> SqliteResult<Vec<u8>> + Send + Sync + 'static,
        decode: impl Fn(&[u8]) -> SqliteResult<T> + Send + Sync + 'static,
    ) -> Self {
        self.encode = Arc::new(encode);
        self.decode = Arc::new(decode);
        self
    }

    /// Returns the declared table name.
    pub fn table(&self) -> &str {
        &self.table
    }
}

/// Cursor used by [`SqliteStore::page`]. It is safe to persist with serde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlitePageCursor {
    /// Model table that owns this cursor.
    pub table: String,
    /// Direction used to create this cursor.
    pub order: SqlitePageOrder,
    /// The encoded ordering key, when the model declares one.
    pub order_key: Option<Vec<u8>>,
    /// The encoded primary key tie-breaker.
    pub id: Vec<u8>,
}

/// Direction for a SQLite keyset page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqlitePageOrder {
    /// Smallest keys first.
    Asc,
    /// Largest keys first.
    Desc,
}

impl SqlitePageOrder {
    fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// A decoded SQLite keyset page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlitePage<T> {
    /// Rows in the requested order.
    pub items: Vec<T>,
    /// Cursor for the next page, if the lookahead found another row.
    pub next: Option<SqlitePageCursor>,
}

struct RawRow {
    id: Vec<u8>,
    order_key: Option<Vec<u8>>,
    payload: Vec<u8>,
}

/// Synchronous SQLite CRUD store with bounded keyset pagination.
pub struct SqliteStore<T> {
    connection: Connection,
    model: SqliteModel<T>,
    table: String,
}

impl<T> SqliteStore<T> {
    /// Opens a SQLite file and creates the compact model table/index.
    pub fn open(path: impl AsRef<Path>, model: SqliteModel<T>) -> SqliteResult<Self> {
        let connection = Connection::open(path).map_err(sqlite_error)?;
        Self::from_connection(connection, model)
    }

    /// Opens an in-memory SQLite database, useful for tests and short-lived tools.
    pub fn open_in_memory(model: SqliteModel<T>) -> SqliteResult<Self> {
        let connection = Connection::open_in_memory().map_err(sqlite_error)?;
        Self::from_connection(connection, model)
    }

    fn from_connection(connection: Connection, model: SqliteModel<T>) -> SqliteResult<Self> {
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .map_err(sqlite_error)?;
        let quoted_table = quote_identifier(model.table())?;
        let index_name = format!("{}_order_id", model.table());
        let quoted_index = quote_identifier(&index_name)?;
        let index_ddl = if model.order.is_some() {
            format!("CREATE INDEX IF NOT EXISTS {quoted_index} ON {quoted_table}(order_key, id);")
        } else {
            String::new()
        };
        connection
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {quoted_table} (\
                    id BLOB NOT NULL PRIMARY KEY,\
                    order_key BLOB,\
                    payload BLOB NOT NULL\
                ) WITHOUT ROWID;\
                {index_ddl}"
            ))
            .map_err(sqlite_error)?;
        Ok(Self {
            connection,
            model,
            table: quoted_table,
        })
    }

    /// Returns the underlying connection for explicit SQLite maintenance.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Inserts a new row and fails on an existing key.
    pub fn create(&self, value: &T) -> SqliteResult<()> {
        let row = self.encode_row(value)?;
        self.connection
            .execute(
                &format!(
                    "INSERT INTO {} (id, order_key, payload) VALUES (?1, ?2, ?3)",
                    self.table
                ),
                params![row.id, row.order_key, row.payload],
            )
            .map(|_| ())
            .map_err(sqlite_error)
    }

    /// Inserts or replaces one row without creating append-only update records.
    pub fn save(&self, value: &T) -> SqliteResult<()> {
        let row = self.encode_row(value)?;
        self.connection
            .execute(
                &format!(
                    "INSERT INTO {} (id, order_key, payload) VALUES (?1, ?2, ?3) \
                     ON CONFLICT(id) DO UPDATE SET order_key = excluded.order_key, payload = excluded.payload",
                    self.table
                ),
                params![row.id, row.order_key, row.payload],
            )
            .map(|_| ())
            .map_err(sqlite_error)
    }

    /// Updates one existing row and fails when the key is absent.
    pub fn update(&self, value: &T) -> SqliteResult<()> {
        let row = self.encode_row(value)?;
        let changed = self
            .connection
            .execute(
                &format!(
                    "UPDATE {} SET order_key = ?2, payload = ?3 WHERE id = ?1",
                    self.table
                ),
                params![row.id, row.order_key, row.payload],
            )
            .map_err(sqlite_error)?;
        if changed == 0 {
            return Err(DBError::NotFound);
        }
        Ok(())
    }

    /// Inserts or updates all rows in one SQLite transaction.
    pub fn save_many(&mut self, values: &[T]) -> SqliteResult<()>
    where
        T: 'static,
    {
        let rows = values
            .iter()
            .map(|value| self.encode_row(value))
            .collect::<SqliteResult<Vec<_>>>()?;
        let transaction = self.connection.transaction().map_err(sqlite_error)?;
        let sql = format!(
            "INSERT INTO {} (id, order_key, payload) VALUES (?1, ?2, ?3) \
             ON CONFLICT(id) DO UPDATE SET order_key = excluded.order_key, payload = excluded.payload",
            self.table
        );
        {
            let mut statement = transaction.prepare(&sql).map_err(sqlite_error)?;
            for row in rows {
                statement
                    .execute(params![row.id, row.order_key, row.payload])
                    .map_err(sqlite_error)?;
            }
        }
        transaction.commit().map_err(sqlite_error)
    }

    /// Reads one row by its typed key.
    pub fn get(&self, key: &SqliteKey) -> SqliteResult<Option<T>> {
        let id = key.to_bytes();
        let row = self
            .connection
            .query_row(
                &format!("SELECT payload FROM {} WHERE id = ?1", self.table),
                params![id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(sqlite_error)?;
        row.map(|payload| (self.model.decode)(&payload)).transpose()
    }

    /// Tests whether at least one row exists.
    pub fn exists(&self) -> SqliteResult<bool> {
        self.connection
            .query_row(
                &format!("SELECT 1 FROM {} LIMIT 1", self.table),
                [],
                |_row| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(sqlite_error)
    }

    /// Deletes one row by its typed key.
    pub fn delete(&self, key: &SqliteKey) -> SqliteResult<bool> {
        let changed = self
            .connection
            .execute(
                &format!("DELETE FROM {} WHERE id = ?1", self.table),
                params![key.to_bytes()],
            )
            .map_err(sqlite_error)?;
        Ok(changed != 0)
    }

    /// Deletes every row while retaining the table and its index.
    pub fn delete_all(&self) -> SqliteResult<()> {
        self.connection
            .execute(&format!("DELETE FROM {}", self.table), [])
            .map(|_| ())
            .map_err(sqlite_error)
    }

    /// Reads the entire table in deterministic key order.
    pub fn list(&self) -> SqliteResult<Vec<T>> {
        let order = if self.model.order.is_some() {
            "ORDER BY order_key ASC, id ASC"
        } else {
            "ORDER BY id ASC"
        };
        let mut statement = self
            .connection
            .prepare(&format!("SELECT payload FROM {} {}", self.table, order))
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(sqlite_error)?;
        rows.map(|row| {
            let payload = row.map_err(sqlite_error)?;
            (self.model.decode)(&payload)
        })
        .collect()
    }

    /// Reads one bounded keyset page. The query never uses OFFSET or a full
    /// table materialization, and the `(order_key,id)` index supplies the tie.
    pub fn page(
        &self,
        count: usize,
        cursor: Option<&SqlitePageCursor>,
        order: SqlitePageOrder,
    ) -> SqliteResult<SqlitePage<T>> {
        if count == 0 {
            return Err(DBError::InvalidModel(
                "SQLite page count must be positive".to_owned(),
            ));
        }
        let limit = count
            .checked_add(1)
            .ok_or_else(|| DBError::InvalidModel("SQLite page count overflowed".to_owned()))?;
        let limit = i64::try_from(limit).map_err(|_| {
            DBError::InvalidModel("SQLite page count exceeds SQLite integer range".to_owned())
        })?;
        let direction = order.sql();
        let ordered = self.model.order.is_some();
        if let Some(cursor) = cursor {
            if cursor.table != self.model.table || cursor.order != order {
                return Err(DBError::InvalidModel(
                    "SQLite page cursor does not match the model or direction".to_owned(),
                ));
            }
        }
        let mut values = Vec::<Value>::new();
        let where_clause = if ordered {
            if let Some(cursor) = cursor {
                let cursor_order = cursor.order_key.as_ref().ok_or_else(|| {
                    DBError::InvalidModel("ordered SQLite page cursor has no order key".to_owned())
                })?;
                values.push(Value::Blob(cursor_order.clone()));
                values.push(Value::Blob(cursor_order.clone()));
                values.push(Value::Blob(cursor.id.clone()));
                if order == SqlitePageOrder::Asc {
                    "WHERE (order_key > ?1 OR (order_key = ?2 AND id > ?3))"
                } else {
                    "WHERE (order_key < ?1 OR (order_key = ?2 AND id < ?3))"
                }
            } else {
                ""
            }
        } else if let Some(cursor) = cursor {
            values.push(Value::Blob(cursor.id.clone()));
            if order == SqlitePageOrder::Asc {
                "WHERE id > ?1"
            } else {
                "WHERE id < ?1"
            }
        } else {
            ""
        };
        values.push(Value::Integer(limit));
        let limit_placeholder = if cursor.is_some() {
            if ordered { "?4" } else { "?2" }
        } else {
            "?1"
        };
        let sql = if ordered {
            format!(
                "SELECT id, order_key, payload FROM {} {} ORDER BY order_key {}, id {} LIMIT {}",
                self.table, where_clause, direction, direction, limit_placeholder
            )
        } else {
            format!(
                "SELECT id, order_key, payload FROM {} {} ORDER BY id {} LIMIT {}",
                self.table, where_clause, direction, limit_placeholder
            )
        };
        let mut statement = self.connection.prepare(&sql).map_err(sqlite_error)?;
        let rows = statement
            .query_map(params_from_iter(values), read_raw_row)
            .map_err(sqlite_error)?;
        let mut raw = Vec::new();
        for row in rows {
            raw.push(row.map_err(sqlite_error)?);
        }
        let has_next = raw.len() > count;
        if has_next {
            raw.pop();
        }
        let next = if has_next {
            let row = raw.last().ok_or(DBError::EmptyResult("SQLite page"))?;
            Some(SqlitePageCursor {
                table: self.model.table.clone(),
                order,
                order_key: row.order_key.clone(),
                id: row.id.clone(),
            })
        } else {
            None
        };
        let items = raw
            .into_iter()
            .map(|row| (self.model.decode)(&row.payload))
            .collect::<SqliteResult<Vec<_>>>()?;
        Ok(SqlitePage { items, next })
    }

    fn encode_row(&self, value: &T) -> SqliteResult<RawRow> {
        let id = (self.model.id)(value).to_bytes();
        let order_key = self
            .model
            .order
            .as_ref()
            .map(|order| order(value).and_then(|value| value.to_bytes()))
            .transpose()?;
        let payload = (self.model.encode)(value)?;
        Ok(RawRow {
            id,
            order_key,
            payload,
        })
    }
}

fn read_raw_row(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: row.get(0)?,
        order_key: row.get(1)?,
        payload: row.get(2)?,
    })
}

fn sqlite_error(error: rusqlite::Error) -> DBError {
    match error {
        rusqlite::Error::QueryReturnedNoRows => DBError::NotFound,
        other => {
            let message = other.to_string();
            if message.to_ascii_lowercase().contains("unique constraint") {
                DBError::Conflict(message)
            } else {
                classify_db_error_text(message).into_db_error()
            }
        }
    }
}
