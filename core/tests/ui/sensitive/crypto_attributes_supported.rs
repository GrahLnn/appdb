use appdb::Sensitive;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Clone, Serialize, Deserialize, SurrealValue, Sensitive)]
#[crypto(service = "billing", account = "tenant-master")]
struct SupportedRecord {
    pub alias: String,

    #[secure]
    pub secret: String,

    #[secure]
    #[crypto(field_account = "tenant-note")]
    pub note: Option<String>,
}

fn main() {
    let fields = SupportedRecord::SECURE_FIELDS;
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].service, Some("billing"));
    assert_eq!(fields[0].account, Some("tenant-master"));
    assert_eq!(fields[1].account, Some("tenant-note"));
}
