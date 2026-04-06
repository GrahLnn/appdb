use super::{Order, QueryKind};
use surrealdb::types::RecordId;

#[test]
fn pagin_uses_bind_placeholders() {
    let sql = QueryKind::pagin(
        "user",
        10,
        Some("abc'; DELETE user; --".to_owned()),
        Order::Asc,
        "id",
    );
    assert!(sql.contains("$cursor"));
    assert!(sql.contains("LIMIT $count"));
}

#[test]
fn relation_lookups_use_bind_placeholders() {
    let in_id = RecordId::new("user", "u1");
    let sql = QueryKind::select_out_ids(&in_id, "follows", "user");
    assert!(sql.contains("FROM $rel"));
    assert!(sql.contains("record::tb(out) = $out_table"));
}

#[test]
fn graph_accessors_use_bind_placeholders() {
    let in_id = RecordId::new("user", "u1");
    let out_sql = QueryKind::select_outgoing_rows(&in_id, "follows", "user");
    assert!(out_sql.contains("LET $ids ="));
    assert!(out_sql.contains("record::tb(out) = $out_table"));
    assert!(out_sql.contains("FROM $ids"));

    let count_sql = QueryKind::count_incoming_in_table(&in_id, "follows", "user");
    assert!(count_sql.contains("count((SELECT VALUE in FROM $rel"));
    assert!(count_sql.contains("record::tb(in) = $in_table"));
}

#[test]
fn relate_uses_relation_insert() {
    let in_id = RecordId::new("task", "t1");
    let out_id = RecordId::new("member", "m1");
    let sql = QueryKind::relate(&in_id, &out_id, "task_assignment");
    assert!(sql.starts_with("INSERT RELATION INTO $rel"));
}

#[test]
fn table_has_rows_returns_a_boolean_probe() {
    let sql = QueryKind::table_has_rows("user");
    assert_eq!(
        sql,
        "RETURN count((SELECT VALUE id FROM $table LIMIT 1)) > 0;"
    );
}
