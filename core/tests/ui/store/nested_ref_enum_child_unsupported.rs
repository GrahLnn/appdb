use appdb::{Bridge, Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
enum PolyChild {
    VariantA { id: Id, name: String },
    VariantB { code: String },
}

fn assert_bridge<T: Bridge>() {}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ParentModel {
    id: Id,
    #[foreign]
    child: PolyChild,
}

fn main() {
    assert_bridge::<PolyChild>();
}
