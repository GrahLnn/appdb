use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[pagin]
    created_at: i64,
    title: String,
}

fn main() {
    let _ = Post::pagin_desc(10, None);
    let _ = Post::pagin_asc(10, None);
}
