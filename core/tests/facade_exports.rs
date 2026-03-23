use appdb::prelude::{Crud, Id, TxStmt};
use appdb::{CryptoContext, Sensitive, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct FacadeSecret {
    #[secure]
    value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct FacadeUser {
    id: Id,
    name: String,
}

#[test]
fn root_and_prelude_exports_expose_capabilities() {
    let stmt = TxStmt::new("RETURN $value;").bind("value", 7i64);
    assert_eq!(stmt.sql, "RETURN $value;");

    let key: Id = "user-1".into();
    assert_eq!(key.as_string(), Some("user-1"));

    let context = CryptoContext::new([9_u8; 32]).expect("context should build");
    let secret = FacadeSecret {
        value: "top-secret".to_owned(),
    };
    let encrypted = secret.encrypt(&context).expect("secret should encrypt");
    assert_ne!(encrypted.value, secret.value.as_bytes());
}

#[test]
fn prelude_reexports_common_items() {
    let stmt = TxStmt::new("RETURN NONE;");
    assert_eq!(stmt.sql, "RETURN NONE;");

    let id: Id = 42.into();
    assert_eq!(id.as_number(), Some(42));

    let _save = FacadeUser::save;
    let _save_many = FacadeUser::save_many;
    let _create = FacadeUser::create;
    let _get = FacadeUser::get::<&str>;
    let _crud_save = <FacadeUser as Crud>::save;
}
