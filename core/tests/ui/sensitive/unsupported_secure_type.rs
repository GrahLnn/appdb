use appdb::Sensitive;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Sensitive)]
struct InvalidRecord {
    #[secure]
    pub counter: std::collections::HashMap<String, String>,
}

fn main() {}
