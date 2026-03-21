use appdb::crypto::{
    clear_crypto_context_registry, register_crypto_context_for, CryptoContext, CryptoError,
    SensitiveModelTag,
};
use appdb::Sensitive;
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex};
use surrealdb::types::SurrealValue;

static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct BankCard {
    pub issuer: String,

    #[secure]
    pub number: String,

    #[secure]
    pub cvv: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct AccountSecrets {
    pub alias: String,

    #[secure]
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct ApiSecrets {
    pub alias: String,

    #[secure]
    pub token: String,
}

#[test]
fn sensitive_record_roundtrip_encrypts_only_secure_fields() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    let ctx = CryptoContext::new([7_u8; 32]).expect("context should build");
    let card = BankCard {
        issuer: "ACME Bank".to_owned(),
        number: "4111111111111111".to_owned(),
        cvv: "123".to_owned(),
    };

    register_crypto_context_for::<AppdbSensitiveFieldTagBankCardNumber>(ctx.clone());
    register_crypto_context_for::<AppdbSensitiveFieldTagBankCardCvv>(ctx.clone());

    let encrypted = card.encrypt(&ctx).expect("record should encrypt");

    assert_eq!(encrypted.issuer, card.issuer);
    assert_ne!(encrypted.number, card.number.as_bytes());
    assert_ne!(encrypted.cvv, card.cvv.as_bytes());

    let decrypted = encrypted.decrypt(&ctx).expect("record should decrypt");
    assert_eq!(decrypted, card);
    clear_crypto_context_registry();
}

#[test]
fn sensitive_runtime_resolver_supports_multi_model_scoping() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    let account_ctx = CryptoContext::new([1_u8; 32]).expect("context should build");
    let api_ctx = CryptoContext::new([2_u8; 32]).expect("context should build");
    register_crypto_context_for::<AppdbSensitiveFieldTagAccountSecretsPassword>(
        account_ctx.clone(),
    );
    register_crypto_context_for::<AppdbSensitiveFieldTagApiSecretsToken>(api_ctx.clone());

    let account = AccountSecrets {
        alias: "main".into(),
        password: "hunter2".into(),
    };
    let api = ApiSecrets {
        alias: "integration".into(),
        token: "secret-token".into(),
    };

    let encrypted_account = account
        .encrypt_with_runtime_resolver()
        .expect("account should encrypt");
    let encrypted_api = api
        .encrypt_with_runtime_resolver()
        .expect("api should encrypt");

    assert_ne!(
        <AccountSecrets as SensitiveModelTag>::model_tag(),
        <ApiSecrets as SensitiveModelTag>::model_tag()
    );
    register_crypto_context_for::<AppdbSensitiveFieldTagAccountSecretsPassword>(api_ctx.clone());
    assert!(matches!(
        AccountSecrets::decrypt_with_runtime_resolver(&encrypted_account),
        Err(CryptoError::Decrypt)
    ));
    register_crypto_context_for::<AppdbSensitiveFieldTagAccountSecretsPassword>(
        account_ctx.clone(),
    );
    assert_eq!(
        AccountSecrets::decrypt_with_runtime_resolver(&encrypted_account)
            .expect("account should decrypt"),
        account
    );
    assert_eq!(
        ApiSecrets::decrypt_with_runtime_resolver(&encrypted_api).expect("api should decrypt"),
        api
    );
    clear_crypto_context_registry();
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct DualSecretCard {
    pub issuer: String,

    #[secure]
    pub number: String,

    #[secure]
    pub cvv: String,
}

#[test]
fn sensitive_runtime_resolver_supports_multi_field_scoping() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    let number_ctx = CryptoContext::new([3_u8; 32]).expect("context should build");
    let cvv_ctx = CryptoContext::new([4_u8; 32]).expect("context should build");
    register_crypto_context_for::<AppdbSensitiveFieldTagDualSecretCardNumber>(number_ctx.clone());
    register_crypto_context_for::<AppdbSensitiveFieldTagDualSecretCardCvv>(cvv_ctx.clone());

    let card = DualSecretCard {
        issuer: "ACME".into(),
        number: "4111111111111111".into(),
        cvv: "123".into(),
    };

    let encrypted = card
        .encrypt_with_runtime_resolver()
        .expect("card should encrypt");
    let number_with_cvv_ctx = appdb::crypto::decrypt_string(&encrypted.number, &cvv_ctx)
        .expect_err("wrong field mapping should fail");

    assert_ne!(encrypted.number, encrypted.cvv);
    assert!(matches!(number_with_cvv_ctx, CryptoError::Decrypt));
    assert_eq!(
        DualSecretCard::decrypt_with_runtime_resolver(&encrypted).expect("card should decrypt"),
        card
    );
    clear_crypto_context_registry();
}

#[test]
fn sensitive_runtime_resolver_reports_missing_mapping() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    let card = BankCard {
        issuer: "ACME Bank".to_owned(),
        number: "4111111111111111".to_owned(),
        cvv: "123".to_owned(),
    };

    let err = card
        .encrypt_with_runtime_resolver()
        .expect_err("missing mapping should fail");

    assert!(matches!(err, CryptoError::ResolverNotFound { .. }));
}
