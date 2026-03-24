use appdb::Sensitive;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
enum DirectSecureEnum {
    Alpha,
    Beta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Sensitive)]
struct InvalidSecureEnumRecord {
    #[secure]
    state: DirectSecureEnum,
}

fn main() {}
