use serde_json::Value;
use surrealdb::types::RecordId;

/// Small SurrealQL builder for the fixed query shapes used by this crate.
pub struct QueryKind;

/// Sort direction for pagination and ordered scans.
pub enum Order {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

impl QueryKind {
    /// Builds a range query over record ids.
    pub fn range(_table: &str, _start: i64, _end: i64) -> String {
        "SELECT * FROM type::record($table, $start)..=type::record($table, $end);".to_owned()
    }
    /// Builds a full-row replace query for a single record.
    pub fn replace(_id: RecordId, _data: Value) -> String {
        "UPDATE $id REPLACE $data;".to_owned()
    }
    /// Builds a keyset pagination query for a table.
    pub fn pagin(
        _table: &str,
        _count: i64,
        cursor: Option<String>,
        order: Order,
        _order_key: &str,
    ) -> String {
        match (cursor.is_some(), order) {
            (true, Order::Asc) => {
                "SELECT * FROM $table WHERE [$order_key] > $cursor ORDER BY [$order_key] ASC LIMIT $count;"
                    .to_owned()
            }
            (true, Order::Desc) => {
                "SELECT * FROM $table WHERE [$order_key] < $cursor ORDER BY [$order_key] DESC LIMIT $count;"
                    .to_owned()
            }
            (false, Order::Asc) => {
                "SELECT * FROM $table ORDER BY [$order_key] ASC LIMIT $count;"
                    .to_owned()
            }
            (false, Order::Desc) => {
                "SELECT * FROM $table ORDER BY [$order_key] DESC LIMIT $count;"
                    .to_owned()
            }
        }
    }
    /// Builds a keyset pagination query scoped to relation rows.
    pub fn rel_pagin(
        _in_id: &RecordId,
        _table: &str,
        _count: i64,
        cursor: Option<String>,
        order: Order,
        _order_key: &str,
    ) -> String {
        let (than, order_str) = match order {
            Order::Asc => (">", "ASC"),
            Order::Desc => ("<", "DESC"),
        };
        match cursor {
            Some(_) => format!(
                "SELECT * FROM $table WHERE [$order_key] {than} $cursor AND in = $in ORDER BY [$order_key] {order_str} LIMIT $count;"
            ),
            None => format!(
                "SELECT * FROM $table WHERE in = $in ORDER BY [$order_key] {order_str} LIMIT $count;"
            ),
        }
    }

    /// Builds a full ordered table scan.
    pub fn all_by_order(_table: &str, order: Order, _key: &str) -> String {
        match order {
            Order::Asc => "SELECT * FROM $table ORDER BY [$key] ASC;".to_owned(),
            Order::Desc => "SELECT * FROM $table ORDER BY [$key] DESC;".to_owned(),
        }
    }
    /// Builds a bounded table scan.
    pub fn limit(_table: &str, _count: i64) -> String {
        "SELECT * FROM $table LIMIT $count;".to_owned()
    }
    /// Builds an `INSERT IGNORE` query for bulk writes.
    pub fn insert(_table: &str) -> String {
        "INSERT IGNORE INTO $table $data;".to_owned()
    }
    /// Builds an insert-or-update query using the provided field names.
    pub fn insert_or_replace(_table: &str, keys: Vec<String>) -> String {
        let mut sql = String::from("INSERT INTO $table $data ON DUPLICATE KEY UPDATE ");
        for (idx, key) in keys.iter().enumerate() {
            if idx > 0 {
                sql.push(',');
            }
            sql.push_str(key);
            sql.push_str("=$input.");
            sql.push_str(key);
        }
        sql.push(';');
        sql
    }
    /// Builds a single-field update query.
    pub fn upsert_set(id: &str, key: &str, _value: &str) -> String {
        let _ = (id, key);
        "UPDATE $id SET [$key] = $value;".to_owned()
    }
    /// Builds a query that returns one record id by field equality.
    pub fn select_id_single(_table: &str) -> String {
        "RETURN (SELECT id FROM ONLY $table WHERE [$k] = $v LIMIT 1).id;".to_owned()
    }
    /// Builds a query that returns up to two record ids by multiple field equalities.
    pub fn select_id_by_fields(fields: &[String]) -> String {
        let where_clause = fields
            .iter()
            .enumerate()
            .map(|(idx, _)| format!("type::field($field_{idx}) = $value_{idx}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        format!("SELECT VALUE id FROM $table WHERE {where_clause} LIMIT 2;")
    }
    /// Builds a query that returns all record ids in a table.
    pub fn all_id(_table: &str) -> String {
        "RETURN (SELECT id FROM $table).id;".to_owned()
    }
    /// Builds a query that returns whether a table contains any rows.
    pub fn table_has_rows(_table: &str) -> String {
        "RETURN count((SELECT VALUE id FROM $table LIMIT 1)) > 0;".to_owned()
    }
    /// Builds a query that projects one field from all rows.
    pub fn single_field(_table: &str, _k: &str) -> String {
        "RETURN (SELECT VALUE [$k] FROM $table);".to_owned()
    }
    /// Builds a query that projects one field from a set of record ids.
    pub fn single_field_by_ids(_ids: Vec<RecordId>, _k: &str) -> String {
        "RETURN (SELECT VALUE [$k] FROM $ids);".to_owned()
    }
    /// Builds a relation insert query.
    pub fn relate(_self_id: &RecordId, _target_id: &RecordId, _rel: &str) -> String {
        "INSERT RELATION INTO $rel [{ in: $in, out: $out, created_at: time::now() }] RETURN NONE;"
            .to_owned()
    }
    /// Builds a query that removes one relation edge.
    pub fn unrelate(_self_id: &RecordId, _target_id: &RecordId, _rel: &str) -> String {
        "DELETE $rel WHERE in = $in AND out = $out RETURN NONE;".to_owned()
    }
    /// Builds a query that removes all outgoing edges for one record.
    pub fn unrelate_all(_self_id: &RecordId, _rel: &str) -> String {
        "DELETE $rel WHERE in = $in RETURN NONE;".to_owned()
    }
    /// Builds a query that returns outgoing record ids for one source record.
    pub fn select_out_ids(_in_id: &RecordId, _rel: &str, _out_table: &str) -> String {
        "RETURN (SELECT VALUE out FROM $rel WHERE in = $in AND record::tb(out) = $out_table);"
            .to_owned()
    }
    /// Builds a query that returns all outgoing record ids for one source record.
    pub fn select_all_out_ids(_in_id: &RecordId, _rel: &str) -> String {
        "RETURN (SELECT VALUE out FROM $rel WHERE in = $in);".to_owned()
    }
    /// Builds a query that returns fully loaded outgoing rows for one source record.
    pub fn select_outgoing_rows(_in_id: &RecordId, _rel: &str, _out_table: &str) -> String {
        "LET $ids = (SELECT VALUE out FROM $rel WHERE in = $in AND record::tb(out) = $out_table); SELECT *, record::id(id) AS id FROM $ids;".to_owned()
    }
    /// Builds a query that counts all outgoing edges for one source record.
    pub fn count_all_outgoing(_in_id: &RecordId, _rel: &str) -> String {
        "RETURN count((SELECT VALUE out FROM $rel WHERE in = $in));".to_owned()
    }
    /// Builds a query that counts outgoing edges for one source record filtered by target table.
    pub fn count_outgoing_in_table(_in_id: &RecordId, _rel: &str, _out_table: &str) -> String {
        "RETURN count((SELECT VALUE out FROM $rel WHERE in = $in AND record::tb(out) = $out_table));"
            .to_owned()
    }
    /// Builds a query that returns ordered outgoing relation edges for one source record.
    pub fn select_out_edges(_in_id: &RecordId, _rel: &str) -> String {
        "SELECT in, out, position FROM $rel WHERE in = $in ORDER BY position ASC;".to_owned()
    }
    /// Builds a query that returns incoming record ids for one target record.
    pub fn select_in_ids(_out_id: &RecordId, _rel: &str, _in_table: &str) -> String {
        "RETURN (SELECT VALUE in FROM $rel WHERE out = $out AND record::tb(in) = $in_table);"
            .to_owned()
    }
    /// Builds a query that returns all incoming record ids for one target record.
    pub fn select_all_in_ids(_out_id: &RecordId, _rel: &str) -> String {
        "RETURN (SELECT VALUE in FROM $rel WHERE out = $out);".to_owned()
    }
    /// Builds a query that returns fully loaded incoming rows for one target record.
    pub fn select_incoming_rows(_out_id: &RecordId, _rel: &str, _in_table: &str) -> String {
        "LET $ids = (SELECT VALUE in FROM $rel WHERE out = $out AND record::tb(in) = $in_table); SELECT *, record::id(id) AS id FROM $ids;".to_owned()
    }
    /// Builds a query that counts all incoming edges for one target record.
    pub fn count_all_incoming(_out_id: &RecordId, _rel: &str) -> String {
        "RETURN count((SELECT VALUE in FROM $rel WHERE out = $out));".to_owned()
    }
    /// Builds a query that counts incoming edges for one target record filtered by source table.
    pub fn count_incoming_in_table(_out_id: &RecordId, _rel: &str, _in_table: &str) -> String {
        "RETURN count((SELECT VALUE in FROM $rel WHERE out = $out AND record::tb(in) = $in_table));"
            .to_owned()
    }
    /// Builds a query that returns one relation row id.
    pub fn rel_id(_self_id: &RecordId, _rel: &str, _target_id: &RecordId) -> String {
        "RETURN (SELECT * FROM ONLY $rel WHERE in = $in AND out = $out LIMIT 1).id;".to_owned()
    }
    /// Builds a create query that only returns the new record id.
    pub fn create_return_id(_table: &str) -> String {
        "RETURN (CREATE ONLY $table CONTENT $data).id;".to_owned()
    }
    /// Builds a record deletion query.
    pub fn delete_record() -> String {
        "DELETE $record RETURN NONE;".to_owned()
    }
    /// Builds a full-table deletion query.
    pub fn delete_table() -> String {
        "DELETE $table RETURN NONE;".to_owned()
    }
    /// Builds a single-record fetch that normalizes the `id` field.
    pub fn select_by_id() -> String {
        "RETURN (SELECT *, record::id(id) AS id FROM ONLY $record);".to_owned()
    }
    /// Builds a full-table fetch that normalizes the `id` field.
    pub fn select_all_with_id() -> String {
        "SELECT *, record::id(id) AS id FROM $table;".to_owned()
    }
    /// Builds a bounded table fetch that normalizes the `id` field.
    pub fn select_limit_with_id() -> String {
        "SELECT *, record::id(id) AS id FROM $table LIMIT $count;".to_owned()
    }
}

#[cfg(test)]
#[path = "builder_tests.rs"]
mod tests;
