use appdb::prelude::*;
use appdb::{Bridge, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct AlphaChild {
    id: Id,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct BetaChild {
    #[unique]
    code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Bridge)]
enum Dispatcher {
    Alpha(AlphaChild),
    Beta(BetaChild),
}

fn assert_bridge<T: appdb::Bridge>() {}

fn main() {
    let _: Dispatcher = AlphaChild {
        id: Id::from("alpha"),
        name: "alpha".to_owned(),
    }
    .into();

    let _: Dispatcher = BetaChild {
        code: "beta".to_owned(),
    }
    .into();

    assert_bridge::<Dispatcher>();
}
