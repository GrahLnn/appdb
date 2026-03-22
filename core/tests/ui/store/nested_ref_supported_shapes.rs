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
    #[foreign]
    child: ChildModel,
    #[foreign]
    maybe_child: Option<ChildModel>,
    #[foreign]
    children: Vec<ChildModel>,
    #[foreign]
    maybe_children: Option<Vec<ChildModel>>,
    #[foreign]
    deeply_nested_children: Option<Vec<Vec<ChildModel>>>,
    #[foreign]
    nested_maybe_children: Vec<Option<ChildModel>>,
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
        maybe_children: Some(Vec::new()),
        deeply_nested_children: Some(vec![vec![ChildModel {
            id: Id::from("deep-child"),
            name: "gamma".to_owned(),
        }]]),
        nested_maybe_children: vec![Some(ChildModel {
            id: Id::from("nested-child"),
            name: "delta".to_owned(),
        })],
        inline_child: ChildModel {
            id: Id::from("inline"),
            name: "beta".to_owned(),
        },
    };
}
