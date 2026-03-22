use appdb::Bridge;
use appdb::prelude::*;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct PlainChild {
    id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Bridge)]
enum Dispatcher {
    Plain(PlainChild),
}

fn main() {}
