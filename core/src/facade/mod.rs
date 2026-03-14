pub mod data {
    pub mod connection {
        pub use crate::connection::*;
    }

    pub mod graph {
        pub use crate::graph::*;
    }

    pub mod model {
        pub mod meta {
            pub use crate::model::meta::*;
        }

        pub mod relation {
            pub use crate::model::relation::*;
        }

        pub mod schema {
            pub use crate::model::schema::*;
        }

        pub use meta::*;
        pub use relation::*;
        pub use schema::*;
    }

    pub mod query {
        pub mod builder {
            pub use crate::query::builder::*;
        }

        pub mod sql {
            pub use crate::query::sql::*;
        }

        pub use builder::*;
        pub use sql::*;
    }

    pub mod repository {
        pub use crate::repository::*;
    }

    pub mod tx {
        pub use crate::tx::*;
    }

    pub use connection::*;
    pub use graph::*;
    pub use model::*;
    pub use query::*;
    pub use repository::*;
    pub use surrealdb::types::{RecordId, Table};
    pub use tx::*;
}

pub mod security {
    pub mod auth {
        pub use crate::auth::*;
    }

    pub mod crypto {
        pub use crate::crypto::*;
    }

    pub use crate::Sensitive;
    pub use auth::*;
    pub use crypto::*;
}

pub mod support {
    pub mod error {
        pub use crate::error::*;
    }

    pub mod id {
        pub use crate::serde_utils::id::*;
    }

    pub use error::*;
    pub use id::*;
}
