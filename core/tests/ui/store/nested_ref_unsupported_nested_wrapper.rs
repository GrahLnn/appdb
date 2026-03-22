use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ChildModel {
    id: Id,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct Wrapper(ChildModel);

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    id: Id,
    #[foreign]
    children: Vec<Wrapper>,
}

fn main() {}
