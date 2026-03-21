use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ChildModel {
    id: Id,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    id: Id,
    #[store(ref)]
    children: Option<Vec<ChildModel>>,
}

fn main() {}
