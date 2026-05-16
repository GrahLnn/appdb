use appdb::{AutoFill, Id, Order, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[pagin]
    #[fill(now)]
    created_at: AutoFill,
    title: String,
}

fn main() {
    let _ = Post::pagin_desc(10, None);
    let _ = Post::pagin_asc(10, None);
    let _ = Post::list().order_by("created_at", Order::Desc);
}
