use appdb::model::meta::ModelMeta;
use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[unique]
    slug: String,
    title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
#[table_as(Post)]
struct PostBase {
    id: Id,
    #[unique]
    slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Feed {
    id: Id,
    #[foreign]
    featured: Option<PostBase>,
    #[foreign]
    nested: Option<Vec<Vec<PostBase>>>,
}

fn main() {
    assert_eq!(Post::table_name(), PostBase::table_name());

    let _ = Feed {
        id: Id::from("feed"),
        featured: Some(PostBase {
            id: Id::from("post"),
            slug: "hello".to_owned(),
        }),
        nested: Some(vec![vec![PostBase {
            id: Id::from("post-2"),
            slug: "world".to_owned(),
        }]]),
    };
}
