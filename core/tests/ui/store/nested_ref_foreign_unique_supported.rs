use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ChildModel {
    #[unique]
    code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    id: Id,
    #[foreign]
    #[unique]
    child: ChildModel,
}

fn main() {
    let _ = ParentModel {
        id: Id::from("parent"),
        child: ChildModel {
            code: "child".to_owned(),
        },
    };
}
