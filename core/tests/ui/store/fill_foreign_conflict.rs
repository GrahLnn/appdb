use appdb::{AutoFill, Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[foreign]
    #[fill(now)]
    created_at: AutoFill,
}

fn main() {}
