use appdb::Sensitive;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Sensitive)]
#[crypto(provider = "nope")]
struct InvalidRecord {
    #[secure]
    pub secret: String,
}

fn main() {}
