use anyhow::{Result, anyhow};
use serde::Serialize;
use surrealdb::opt::auth::Record;
use surrealdb::types::SurrealValue;

use crate::connection::get_db;

#[derive(Serialize, SurrealValue)]
/// Credentials used for the built-in root account bootstrap flow.
struct RootCredentials {
    user: String,
    pass: String,
}

fn root_user(pass: &str) -> Record<RootCredentials> {
    Record {
        namespace: "app".to_owned(),
        database: "app".to_owned(),
        access: "account".to_owned(),
        params: RootCredentials {
            user: "root".to_owned(),
            pass: pass.to_owned(),
        },
    }
}

/// Ensures the default root record-access account exists and is usable.
pub async fn ensure_root_user(pass: &str) -> Result<()> {
    let db = get_db()?;
    ensure_root_user_with(
        pass,
        |creds| async { db.signin(creds).await.map(|_| ()).map_err(Into::into) },
        |creds| async { db.signup(creds).await.map(|_| ()).map_err(Into::into) },
    )
    .await
}

async fn ensure_root_user_with<Signin, SigninFuture, Signup, SignupFuture>(
    pass: &str,
    signin: Signin,
    signup: Signup,
) -> Result<()>
where
    Signin: FnOnce(Record<RootCredentials>) -> SigninFuture,
    SigninFuture: Future<Output = Result<()>>,
    Signup: FnOnce(Record<RootCredentials>) -> SignupFuture,
    SignupFuture: Future<Output = Result<()>>,
{
    match signin(root_user(pass)).await {
        Ok(_) => Ok(()),
        Err(signin_err) => signup(root_user(pass)).await.map_err(|signup_err| {
            anyhow!(
                "root record access signin failed: {signin_err}; signup fallback failed: {signup_err}"
            )
        }),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
