use appdb::Sensitive;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Sensitive)]
struct InvalidRecord {
    #[crypto(field_account = "tenant-note")]
    pub secret: String,
}

fn main() {}
