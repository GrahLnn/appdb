use appdb::prelude::*;
use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
pub struct Child {
    pub id: Id,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
pub struct Parent {
    pub id: Id,
    #[foreign]
    pub child: Child,
}

fn assert_stored_model<T: appdb::StoredModel>() {}

fn main() {
    assert_stored_model::<Parent>();
}
