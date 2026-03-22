use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
#[table_as(Post)]
#[table_as(Post)]
struct PostBase {
    id: Id,
}

fn main() {}
