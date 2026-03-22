use appdb::Bridge;
use appdb::prelude::*;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct Child {
    id: Id,
}

#[derive(Bridge)]
enum Dispatcher {
    Pair(Child, Child),
}

fn main() {}
