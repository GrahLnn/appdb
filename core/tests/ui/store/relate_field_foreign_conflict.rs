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
    #[foreign]
    #[relate("edge_conflict")]
    child: RelatedLeaf,
}

fn main() {}
