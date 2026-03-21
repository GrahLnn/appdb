use appdb::{Sensitive, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store, Sensitive)]
struct InvalidSecureUniqueOrder {
    #[secure]
    #[unique]
    secret: String,
}

fn main() {}
