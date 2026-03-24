use appdb::Sensitive;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Clone, Serialize, Deserialize, SurrealValue, Sensitive)]
struct ChildSecrets {
    #[secure]
    secret: String,
}

#[derive(Clone, Serialize, Deserialize, SurrealValue, Sensitive)]
struct ParentSecrets {
    alias: String,

    #[secure]
    child: ChildSecrets,

    #[secure]
    maybe_child: Option<ChildSecrets>,

    #[secure]
    child_list: Vec<ChildSecrets>,
}

fn main() {
    let _ = ParentSecrets::SECURE_FIELDS;
}
