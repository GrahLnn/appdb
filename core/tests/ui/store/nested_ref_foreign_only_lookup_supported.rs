use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ChildModel {
    #[unique]
    code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    #[foreign]
    child: ChildModel,
}

fn main() {
    let _ = ParentModel {
        child: ChildModel {
            code: "child".to_owned(),
        },
    };
}
