use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[pagin]
    created_at: Option<i64>,
}

fn main() {}
