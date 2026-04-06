use super::extract_record_id_key;
use crate::model::meta::ModelMeta;
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordIdKey, SurrealValue};

#[derive(Serialize)]
struct GoodModel {
    id: String,
}

#[derive(Serialize)]
struct MissingId {
    name: String,
}

#[derive(Serialize)]
struct BadIdType {
    id: bool,
}

#[derive(Serialize)]
struct NumberIdType {
    id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct AutoTableModel {
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct CustomTableModel {
    id: String,
}

impl ModelMeta for AutoTableModel {
    fn table_name() -> &'static str {
        static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        TABLE_NAME.get_or_init(|| {
            let table = crate::model::meta::default_table_name(stringify!(AutoTableModel));
            crate::model::meta::register_table(stringify!(AutoTableModel), table)
        })
    }
}

impl ModelMeta for CustomTableModel {
    fn table_name() -> &'static str {
        static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        TABLE_NAME.get_or_init(|| {
            crate::model::meta::register_table(stringify!(CustomTableModel), "custom_users")
        })
    }
}

impl crate::StoredModel for AutoTableModel {
    type Stored = Self;

    fn into_stored(self) -> anyhow::Result<Self::Stored> {
        Ok(self)
    }

    fn from_stored(stored: Self::Stored) -> anyhow::Result<Self> {
        Ok(stored)
    }
}

impl crate::StoredModel for CustomTableModel {
    type Stored = Self;

    fn into_stored(self) -> anyhow::Result<Self::Stored> {
        Ok(self)
    }

    fn from_stored(stored: Self::Stored) -> anyhow::Result<Self> {
        Ok(stored)
    }
}

impl crate::ForeignModel for AutoTableModel {
    async fn persist_foreign(value: Self) -> anyhow::Result<Self::Stored> {
        Ok(value)
    }

    async fn hydrate_foreign(stored: Self::Stored) -> anyhow::Result<Self> {
        Ok(stored)
    }
}

impl crate::ForeignModel for CustomTableModel {
    async fn persist_foreign(value: Self) -> anyhow::Result<Self::Stored> {
        Ok(value)
    }

    async fn hydrate_foreign(stored: Self::Stored) -> anyhow::Result<Self> {
        Ok(stored)
    }
}

impl crate::repository::Crud for AutoTableModel {}
impl crate::repository::Crud for CustomTableModel {}

#[test]
fn extract_id_succeeds_for_valid_model() {
    let model = GoodModel {
        id: "u1".to_owned(),
    };
    assert_eq!(
        extract_record_id_key(&model).expect("expected id"),
        RecordIdKey::String("u1".to_owned())
    );
}

#[test]
fn extract_number_id_succeeds_for_valid_model() {
    let model = NumberIdType { id: 42 };
    assert_eq!(
        extract_record_id_key(&model).expect("expected id"),
        RecordIdKey::Number(42)
    );
}

#[test]
fn extract_id_fails_when_id_missing() {
    let model = MissingId {
        name: "alice".to_owned(),
    };
    let err = extract_record_id_key(&model).expect_err("expected missing id error");
    assert!(err.to_string().contains("does not contain an `id`"));
}

#[test]
fn extract_id_fails_when_id_not_string_or_number() {
    let model = BadIdType { id: true };
    let err = extract_record_id_key(&model).expect_err("expected bad id type error");
    assert!(
        err.to_string()
            .contains("not a non-empty string or i64 number")
    );
}

#[test]
fn extract_id_fails_when_id_empty() {
    let model = GoodModel { id: String::new() };
    let err = extract_record_id_key(&model).expect_err("expected empty id error");
    assert!(
        err.to_string()
            .contains("not a non-empty string or i64 number")
    );
}

#[test]
fn default_table_name_from_impl_crud_is_applied() {
    assert_eq!(
        <AutoTableModel as ModelMeta>::table_name(),
        "auto_table_model"
    );
}

#[test]
fn custom_table_name_from_impl_crud_override_is_applied() {
    assert_eq!(
        <CustomTableModel as ModelMeta>::table_name(),
        "custom_users"
    );
}
