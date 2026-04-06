use super::{
    Id, deserialize_id_or_record_id_as_string, deserialize_record_id_or_compat_string,
    serialize_id_as_string,
};
use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId;

#[derive(Debug, Deserialize)]
struct RawId {
    #[serde(deserialize_with = "deserialize_id_or_record_id_as_string")]
    id: String,
}

#[derive(Debug, Serialize)]
struct OutId {
    #[serde(serialize_with = "serialize_id_as_string")]
    id: String,
}

#[derive(Debug, Deserialize)]
struct WrappedId {
    id: Id,
}

#[derive(Debug, Deserialize)]
struct WrappedRecordId {
    #[serde(deserialize_with = "deserialize_record_id_or_compat_string")]
    id: RecordId,
}

#[derive(Debug, Serialize)]
struct OutWrappedId {
    id: Id,
}

#[test]
fn deserializes_id_string() {
    let row: RawId = serde_json::from_str(r#"{"id":"alice"}"#).expect("must deserialize");
    assert_eq!(row.id, "alice");
}

#[test]
fn deserializes_number_id_into_string() {
    let row: RawId = serde_json::from_str(r#"{"id":42}"#).expect("must deserialize");
    assert_eq!(row.id, "42");
}

#[test]
fn deserializes_record_id() {
    let record = RecordId::new("user", "alice");
    let json = format!(r#"{{"id":{}}}"#, serde_json::to_string(&record).unwrap());
    let row: RawId = serde_json::from_str(&json).expect("must deserialize record id");
    assert_eq!(row.id, "alice");
}

#[test]
fn serializes_record_id_shape() {
    let record = RecordId::new("user", "alice");
    let json = serde_json::to_value(&record).expect("record id should serialize");
    assert_eq!(
        json,
        serde_json::json!({ "table": "user", "key": { "String": "alice" } })
    );
}

#[test]
fn compat_record_id_accepts_table_agnostic_string_form() {
    let row: WrappedRecordId = serde_json::from_str(r#"{"id":"custom_table:alice"}"#)
        .expect("must deserialize string-form record id");
    assert_eq!(row.id, RecordId::new("custom_table", "alice"));
}

#[test]
fn serializes_id_string() {
    let row = OutId {
        id: "alice".to_owned(),
    };
    let json = serde_json::to_string(&row).expect("must serialize");
    assert_eq!(json, r#"{"id":"alice"}"#);
}

#[test]
fn id_accepts_plain_string() {
    let row: WrappedId = serde_json::from_str(r#"{"id":"alice"}"#).expect("must deserialize");
    assert_eq!(row.id, Id::String("alice".to_owned()));
}

#[test]
fn id_accepts_plain_number() {
    let row: WrappedId = serde_json::from_str(r#"{"id":42}"#).expect("must deserialize");
    assert_eq!(row.id, Id::Number(42));
}

#[test]
fn id_accepts_record_id_string() {
    let record = RecordId::new("user", "alice");
    let json = format!(r#"{{"id":{}}}"#, serde_json::to_string(&record).unwrap());
    let row: WrappedId = serde_json::from_str(&json).expect("must deserialize record id");
    assert_eq!(row.id, Id::String("alice".to_owned()));
}

#[test]
fn id_accepts_record_id_number() {
    let record = RecordId::new("user", 42i64);
    let json = format!(r#"{{"id":{}}}"#, serde_json::to_string(&record).unwrap());
    let row: WrappedId = serde_json::from_str(&json).expect("must deserialize record id");
    assert_eq!(row.id, Id::Number(42));
}

#[test]
fn id_serializes_string_as_string() {
    let row = OutWrappedId {
        id: Id::from("alice"),
    };
    let json = serde_json::to_string(&row).expect("must serialize");
    assert_eq!(json, r#"{"id":"alice"}"#);
}

#[test]
fn id_serializes_number_as_number() {
    let row = OutWrappedId {
        id: Id::from(42i64),
    };
    let json = serde_json::to_string(&row).expect("must serialize");
    assert_eq!(json, r#"{"id":42}"#);
}
