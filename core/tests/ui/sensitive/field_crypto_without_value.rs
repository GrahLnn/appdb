use appdb::Sensitive;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Sensitive)]
struct InvalidRecord {
    #[secure]
    #[crypto(field_account)]
    pub secret: String,
}

fn main() {}
