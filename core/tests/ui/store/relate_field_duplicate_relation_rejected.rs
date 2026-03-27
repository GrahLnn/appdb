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
    #[relate("edge_shared")]
    left: RelatedLeaf,
    #[relate("edge_shared")]
    right: Vec<RelatedLeaf>,
}

fn main() {}
