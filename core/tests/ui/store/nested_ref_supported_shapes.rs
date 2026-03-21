use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ChildModel {
    id: Id,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    id: Id,
    #[store(ref)]
    child: ChildModel,
    #[store(ref)]
    maybe_child: Option<ChildModel>,
    #[store(ref)]
    children: Vec<ChildModel>,
    inline_child: ChildModel,
}

fn main() {
    let _ = ParentModel {
        id: Id::from("parent"),
        child: ChildModel {
            id: Id::from("child"),
            name: "alpha".to_owned(),
        },
        maybe_child: None,
        children: Vec::new(),
        inline_child: ChildModel {
            id: Id::from("inline"),
            name: "beta".to_owned(),
        },
    };
}
