use appdb::Sensitive;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Sensitive)]
struct InvalidRecord {
    #[secure]
    #[crypto(account = "wrong-place")]
    pub secret: String,
}

fn main() {}
