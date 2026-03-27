use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct RelatedLeaf {
    id: Id,
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct RelatedRoot {
    id: Id,
    #[unique]
    #[relate("edge_unique")]
    child: RelatedLeaf,
}

fn main() {}
