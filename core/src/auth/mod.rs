use anyhow::{anyhow, Result};
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

    match db.signin(root_user(pass)).await {
        Ok(_) => Ok(()),
        Err(signin_err) => db.signup(root_user(pass)).await.map(|_| ()).map_err(|signup_err| {
            anyhow!(
                "root record access signin failed: {signin_err}; signup fallback failed: {signup_err}"
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::root_user;

    #[test]
    fn root_user_uses_expected_record_access_shape() {
        let record = root_user("secret");
        assert_eq!(record.namespace, "app");
        assert_eq!(record.database, "app");
        assert_eq!(record.access, "account");
    }
}
