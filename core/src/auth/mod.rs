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
mod tests {
    use std::cell::RefCell;
    use std::future::{Ready, ready};
    use std::rc::Rc;

    use anyhow::{Result, anyhow};

    use super::{ensure_root_user_with, root_user};

    #[test]
    fn root_user_uses_expected_record_access_shape() {
        let record = root_user("secret");
        assert_eq!(record.namespace, "app");
        assert_eq!(record.database, "app");
        assert_eq!(record.access, "account");
    }

    #[tokio::test]
    async fn ensure_root_user_successful_signin_skips_signup() {
        let signup_calls = Rc::new(RefCell::new(0usize));
        let signin_passwords = Rc::new(RefCell::new(Vec::new()));
        let signup_passwords = Rc::new(RefCell::new(Vec::new()));

        ensure_root_user_with(
            "secret",
            {
                let signin_passwords = Rc::clone(&signin_passwords);
                move |record| {
                    signin_passwords
                        .borrow_mut()
                        .push(record.params.pass.clone());
                    ready(Ok(()))
                }
            },
            {
                let signup_calls = Rc::clone(&signup_calls);
                let signup_passwords = Rc::clone(&signup_passwords);
                move |record| {
                    *signup_calls.borrow_mut() += 1;
                    signup_passwords
                        .borrow_mut()
                        .push(record.params.pass.clone());
                    ready(Ok(()))
                }
            },
        )
        .await
        .expect("successful signin should return ok");

        assert_eq!(
            signin_passwords.borrow().as_slice(),
            ["secret"],
            "signin should receive the requested password exactly once"
        );
        assert_eq!(
            *signup_calls.borrow(),
            0,
            "successful signin must not trigger signup"
        );
        assert!(
            signup_passwords.borrow().is_empty(),
            "signup should not receive credentials when signin succeeds"
        );
    }

    #[tokio::test]
    async fn ensure_root_user_signin_failure_falls_back_to_single_signup_attempt() {
        let signin_passwords = Rc::new(RefCell::new(Vec::new()));
        let signup_calls = Rc::new(RefCell::new(0usize));
        let signup_passwords = Rc::new(RefCell::new(Vec::new()));

        ensure_root_user_with(
            "fallback-pass",
            {
                let signin_passwords = Rc::clone(&signin_passwords);
                move |record| {
                    signin_passwords
                        .borrow_mut()
                        .push(record.params.pass.clone());
                    ready(Err(anyhow!("signin failed")))
                }
            },
            {
                let signup_calls = Rc::clone(&signup_calls);
                let signup_passwords = Rc::clone(&signup_passwords);
                move |record| {
                    *signup_calls.borrow_mut() += 1;
                    signup_passwords
                        .borrow_mut()
                        .push(record.params.pass.clone());
                    ready(Ok(()))
                }
            },
        )
        .await
        .expect("signup fallback should recover from signin failure");

        assert_eq!(
            signin_passwords.borrow().as_slice(),
            ["fallback-pass"],
            "signin should still run exactly once before fallback"
        );
        assert_eq!(
            *signup_calls.borrow(),
            1,
            "signin failure should trigger exactly one signup attempt"
        );
        assert_eq!(
            signup_passwords.borrow().as_slice(),
            ["fallback-pass"],
            "signup fallback should reuse the requested password"
        );
    }

    #[tokio::test]
    async fn ensure_root_user_dual_failure_preserves_signin_and_signup_context() {
        let signup_calls = Rc::new(RefCell::new(0usize));

        let err = ensure_root_user_with(
            "broken-pass",
            move |_| -> Ready<Result<()>> { ready(Err(anyhow!("signin refused root"))) },
            {
                let signup_calls = Rc::clone(&signup_calls);
                move |_| {
                    *signup_calls.borrow_mut() += 1;
                    ready(Err(anyhow!("signup refused root")))
                }
            },
        )
        .await
        .expect_err("dual failure should return a combined error");

        let message = err.to_string();
        assert_eq!(
            *signup_calls.borrow(),
            1,
            "dual failure should still attempt signup exactly once"
        );
        assert!(
            message.contains("signin refused root"),
            "combined error should keep signin context: {message}"
        );
        assert!(
            message.contains("signup refused root"),
            "combined error should keep signup context: {message}"
        );
    }
}
