use anyhow::Result;
use serde_json::Value;
use surrealdb::types::{Number, RecordId, Table, Value as SurrealDbValue};

use crate::Id;
use crate::error::DBError;
use crate::query::builder::{Order, QueryKind};
use crate::query::sql::RawSqlStmt;

/// One cursor-paginated result page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Page<T> {
    /// Materialized rows for the current page.
    pub items: Vec<T>,
    /// Cursor that resumes immediately after the last returned row.
    pub next: Option<PageCursor>,
}

/// Opaque cursor token used by Store pagination helpers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PageCursor(String);

impl PageCursor {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    fn encode(payload: PageCursorPayload) -> Result<Self> {
        Ok(Self(serde_json::to_string(&payload)?))
    }

    fn decode(&self) -> Result<PageCursorPayload> {
        serde_json::from_str(&self.0)
            .map_err(|err| DBError::Decode(format!("failed to decode page cursor: {err}")).into())
    }
}

impl AsRef<str> for PageCursor {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<PageCursor> for String {
    fn from(value: PageCursor) -> Self {
        value.0
    }
}

const PAGE_CURSOR_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum CursorOrder {
    Asc,
    Desc,
}

impl From<Order> for CursorOrder {
    fn from(value: Order) -> Self {
        match value {
            Order::Asc => Self::Asc,
            Order::Desc => Self::Desc,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PageCursorPayload {
    version: u8,
    field: String,
    order: CursorOrder,
    value: serde_json::Value,
    id: Id,
}

struct DecodedPageCursor {
    value: SurrealDbValue,
    id: Id,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PaginationPlan<'a> {
    field: &'a str,
    order: Order,
}

impl<'a> PaginationPlan<'a> {
    pub(crate) fn new(field: &'a str, order: Order) -> Self {
        Self { field, order }
    }

    pub(crate) fn build_stmt(
        self,
        table: &'static str,
        count: i64,
        cursor: Option<&PageCursor>,
    ) -> Result<RawSqlStmt> {
        let mut stmt = RawSqlStmt::new(QueryKind::pagin(
            table,
            count,
            cursor.is_some(),
            self.order,
            self.query_order_key(),
        ))
        .bind("table", Table::from(table))
        .bind("count", count);

        if let Some(cursor) = cursor {
            let decoded = self.decode_cursor(cursor)?;
            stmt = stmt
                .bind("cursor_value", decoded.value)
                .bind("cursor_record", RecordId::new(table, decoded.id));
        }

        Ok(stmt)
    }

    pub(crate) fn build_cursor(self, row: &serde_json::Value) -> Result<PageCursor> {
        PageCursor::encode(PageCursorPayload {
            version: PAGE_CURSOR_VERSION,
            field: self.field.to_owned(),
            order: CursorOrder::from(self.order),
            value: self.cursor_value_from_row(row)?,
            id: self.id_from_row(row)?,
        })
    }

    fn query_order_key(self) -> &'a str {
        if self.field == "id" {
            "__page_public_id"
        } else {
            self.field
        }
    }

    fn decode_cursor(self, cursor: &PageCursor) -> Result<DecodedPageCursor> {
        let payload = cursor.decode()?;

        if payload.version != PAGE_CURSOR_VERSION {
            return Err(DBError::Decode(format!(
                "unsupported page cursor version `{}`",
                payload.version
            ))
            .into());
        }

        if payload.field != self.field {
            return Err(DBError::Decode(format!(
                "page cursor targets `{}` but `{}` was requested",
                payload.field, self.field
            ))
            .into());
        }

        if payload.order != CursorOrder::from(self.order) {
            return Err(DBError::Decode(
                "page cursor order does not match the requested pagination direction".to_owned(),
            )
            .into());
        }

        let value = cursor_value_to_surreal(payload.value).map_err(|err| {
            let typed = DBError::from(err);
            DBError::Decode(format!(
                "failed to decode {} page cursor value for `{}`: {}",
                self.order_label(),
                self.field,
                typed
            ))
        })?;

        Ok(DecodedPageCursor {
            value,
            id: payload.id,
        })
    }

    fn order_label(self) -> &'static str {
        match self.order {
            Order::Asc => "ascending",
            Order::Desc => "descending",
        }
    }

    fn cursor_value_from_row(self, row: &serde_json::Value) -> Result<serde_json::Value> {
        row.as_object()
            .and_then(|map| map.get(self.field))
            .cloned()
            .ok_or_else(|| {
                DBError::Decode(format!(
                    "stored row is missing pagination field `{}`",
                    self.field
                ))
                .into()
            })
    }

    fn id_from_row(self, row: &serde_json::Value) -> Result<Id> {
        let id = row
            .as_object()
            .and_then(|map| map.get("id"))
            .cloned()
            .ok_or_else(|| DBError::Decode("stored row is missing `id`".to_owned()))?;

        serde_json::from_value(id).map_err(|err| {
            DBError::Decode(format!("failed to decode pagination row id: {err}")).into()
        })
    }
}

fn cursor_value_to_surreal(value: Value) -> Result<SurrealDbValue> {
    match value {
        Value::Null => Ok(SurrealDbValue::Null),
        Value::Bool(value) => Ok(SurrealDbValue::Bool(value)),
        Value::String(value) => Ok(SurrealDbValue::String(value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(SurrealDbValue::Number(Number::Int(value)))
            } else if let Some(value) = value.as_u64() {
                let value = i64::try_from(value).map_err(|_| {
                    DBError::Decode("page cursor numeric value exceeds i64 range".to_owned())
                })?;
                Ok(SurrealDbValue::Number(Number::Int(value)))
            } else if let Some(value) = value.as_f64() {
                Ok(SurrealDbValue::Number(Number::Float(value)))
            } else {
                Err(
                    DBError::Decode("page cursor number could not be represented".to_owned())
                        .into(),
                )
            }
        }
        Value::Object(_) => {
            let record: RecordId = serde_json::from_value(value).map_err(|err| {
                DBError::Decode(format!("failed to decode page cursor record id: {err}"))
            })?;
            Ok(SurrealDbValue::RecordId(record))
        }
        Value::Array(_) => Err(DBError::Decode(
            "page cursor cannot bind array values; #[pagin] only supports scalar fields".to_owned(),
        )
        .into()),
    }
}
