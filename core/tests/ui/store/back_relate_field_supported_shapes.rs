use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct RelatedLeaf {
    id: Id,
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct RelatedRoot {
    id: Id,
    title: String,
    #[back_relate("edge_single")]
    single: RelatedLeaf,
    #[back_relate("edge_optional")]
    optional: Option<RelatedLeaf>,
    #[back_relate("edge_many")]
    many: Vec<RelatedLeaf>,
    #[back_relate("edge_optional_many")]
    optional_many: Option<Vec<RelatedLeaf>>,
}

fn main() {
    let _ = RelatedRoot {
        id: Id::from("root"),
        title: "alpha".to_owned(),
        single: RelatedLeaf {
            id: Id::from("leaf-single"),
            label: "single".to_owned(),
        },
        optional: Some(RelatedLeaf {
            id: Id::from("leaf-optional"),
            label: "optional".to_owned(),
        }),
        many: vec![
            RelatedLeaf {
                id: Id::from("leaf-many-1"),
                label: "many-1".to_owned(),
            },
            RelatedLeaf {
                id: Id::from("leaf-many-2"),
                label: "many-2".to_owned(),
            },
        ],
        optional_many: Some(vec![RelatedLeaf {
            id: Id::from("leaf-optional-many-1"),
            label: "optional-many-1".to_owned(),
        }]),
    };
}
