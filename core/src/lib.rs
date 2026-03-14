extern crate self as appdb;

pub mod auth;
pub mod connection;
pub mod crypto;
pub mod error;
pub mod facade;
pub mod graph;
pub mod model;
pub mod prelude;
pub mod query;
pub mod repository;
pub mod serde_utils;
pub mod tx;

pub use appdb_macros::Sensitive;
pub use facade::data::*;
pub use facade::security::{auth::*, crypto::*};
pub use facade::support::{error::*, id::*};

pub trait Sensitive: Sized {
    type Encrypted;

    fn encrypt(
        &self,
        context: &crate::crypto::CryptoContext,
    ) -> Result<Self::Encrypted, crate::crypto::CryptoError>;

    fn decrypt(
        encrypted: &Self::Encrypted,
        context: &crate::crypto::CryptoContext,
    ) -> Result<Self, crate::crypto::CryptoError>;
}
