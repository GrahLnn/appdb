use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct PlainChild {
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    id: Id,
    #[store(ref)]
    child: PlainChild,
}

fn main() {}
