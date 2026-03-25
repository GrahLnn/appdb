use appdb::crypto::{
    CryptoContext, CryptoError, SensitiveFieldTag, SensitiveModelTag,
    clear_crypto_context_registry, default_crypto_config, register_crypto_context_for,
    reset_default_crypto_config, set_default_crypto_account, set_default_crypto_config,
    set_default_crypto_service,
};
use appdb::{Sensitive, SensitiveShape, SensitiveValueOf};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use surrealdb::types::SurrealValue;

static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn crypto_test_local_appdata(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "appdb_sensitive_crypto_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ))
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
#[crypto(service = "billing", account = "tenant-master")]
struct OverrideSecrets {
    pub alias: String,

    #[secure]
    pub api_key: String,

    #[secure]
    #[crypto(field_account = "tenant-note")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct FirstUseDefaultsA {
    pub alias: String,

    #[secure]
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct FirstUseDefaultsB {
    pub alias: String,

    #[secure]
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct NestedSensitiveChild {
    pub label: String,

    #[secure]
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct NestedSensitiveParent {
    pub alias: String,

    #[secure]
    pub child: NestedSensitiveChild,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct OptionalNestedSensitiveParent {
    pub alias: String,

    #[secure]
    pub child: Option<NestedSensitiveChild>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct OverrideNestedSensitiveLeaf {
    pub label: String,

    #[secure]
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum RuntimeSeamStatus {
    Draft,
    Published,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum RuntimeSeamState {
    Draft { note: String },
    Published { version: u32, tags: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RuntimeSeamPayload {
    pub alias: String,
    pub status: RuntimeSeamStatus,
    pub optional_status: Option<RuntimeSeamStatus>,
    pub state: RuntimeSeamState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
struct RuntimeSeamParent {
    pub alias: String,

    #[secure]
    pub payload: SensitiveValueOf<RuntimeSeamPayload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
#[crypto(service = "nested-runtime", account = "runtime-master")]
struct OverrideNestedSensitiveParent {
    pub alias: String,

    #[secure]
    #[crypto(field_account = "runtime-left")]
    pub left: OverrideNestedSensitiveLeaf,

    #[secure]
    #[crypto(field_account = "runtime-right")]
    pub right: OverrideNestedSensitiveLeaf,
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
fn sensitive_runtime_resolver_auto_initializes_from_defaults() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("defaults");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }
    let card = BankCard {
        issuer: "ACME Bank".to_owned(),
        number: "4111111111111111".to_owned(),
        cvv: "123".to_owned(),
    };

    let encrypted = card
        .encrypt_with_runtime_resolver()
        .expect("defaults should auto-register runtime contexts");
    let decrypted = BankCard::decrypt_with_runtime_resolver(&encrypted)
        .expect("auto-registered contexts should decrypt");

    assert_eq!(decrypted, card);
    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_runtime_resolver_first_use_initialization_is_single_flight_per_model() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("single-flight-runtime");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Sensitive)]
    struct RuntimeSingleFlightSecrets {
        pub alias: String,

        #[secure]
        pub secret: String,
    }

    let alpha = RuntimeSingleFlightSecrets {
        alias: "alpha".into(),
        secret: "one".into(),
    };
    let beta = RuntimeSingleFlightSecrets {
        alias: "beta".into(),
        secret: "two".into(),
    };

    let first = std::thread::spawn({
        let value = alpha.clone();
        move || value.encrypt_with_runtime_resolver()
    });
    let second = std::thread::spawn({
        let value = beta.clone();
        move || value.encrypt_with_runtime_resolver()
    });

    let encrypted_alpha = first
        .join()
        .expect("first runtime worker should not panic")
        .expect("first runtime call should initialize crypto");
    let encrypted_beta = second
        .join()
        .expect("second runtime worker should not panic")
        .expect("second runtime call should reuse initialization");

    set_default_crypto_config("single-flight-mutated-svc", "single-flight-mutated-acct");

    assert_eq!(
        RuntimeSingleFlightSecrets::decrypt_with_runtime_resolver(&encrypted_alpha)
            .expect("first encrypted value should decrypt with established context"),
        alpha
    );
    assert_eq!(
        RuntimeSingleFlightSecrets::decrypt_with_runtime_resolver(&encrypted_beta)
            .expect("second encrypted value should decrypt with established context"),
        beta
    );

    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_runtime_resolver_first_use_models_track_global_default_changes_without_retroactive_leakage()
 {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("first-use-default-mutation");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }

    set_default_crypto_config("svc-initial", "acct-initial");
    let initial = FirstUseDefaultsA {
        alias: "alpha".into(),
        secret: "one".into(),
    };
    let encrypted_initial = initial
        .encrypt_with_runtime_resolver()
        .expect("first model should auto-initialize from initial defaults");

    set_default_crypto_config("svc-later", "acct-later");
    let later = FirstUseDefaultsB {
        alias: "beta".into(),
        secret: "two".into(),
    };
    let encrypted_later = later
        .encrypt_with_runtime_resolver()
        .expect("later first-use model should auto-initialize from mutated defaults");

    assert_eq!(
        FirstUseDefaultsA::decrypt_with_runtime_resolver(&encrypted_initial)
            .expect("already-initialized model should still decrypt with original context"),
        initial
    );
    assert_eq!(
        FirstUseDefaultsB::decrypt_with_runtime_resolver(&encrypted_later)
            .expect("later model should decrypt with later defaults"),
        later
    );
    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_runtime_resolver_auto_initialization_failures_surface_crypto_errors() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("invalid-key");
    let backup_dir = local_appdata.join("broken_service").join("key-backup");
    std::fs::create_dir_all(&backup_dir).expect("backup dir should be creatable");
    std::fs::write(backup_dir.join("broken_account.bin"), [1_u8; 7])
        .expect("invalid backup key should be written");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }
    set_default_crypto_config("broken_service", "broken_account");

    let card = BankCard {
        issuer: "ACME Bank".to_owned(),
        number: "4111111111111111".to_owned(),
        cvv: "123".to_owned(),
    };

    let err = card
        .encrypt_with_runtime_resolver()
        .expect_err("invalid provider key should surface a crypto error");

    assert!(
        matches!(
            err,
            CryptoError::InvalidKeyLength
                | CryptoError::SecretStore(_)
                | CryptoError::ProtectedBackup(_)
        ),
        "unexpected error: {err:?}"
    );
    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_crypto_metadata_exposes_defaults_and_overrides() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_default_crypto_config();
    set_default_crypto_config("svc-a", "acct-a");

    let defaults = default_crypto_config();
    assert_eq!(defaults.service, "svc-a");
    assert_eq!(defaults.account, "acct-a");

    let account_meta =
        <AppdbSensitiveFieldTagAccountSecretsPassword as SensitiveFieldTag>::crypto_metadata();
    assert_eq!(
        account_meta.model_tag,
        <AccountSecrets as SensitiveModelTag>::model_tag()
    );
    assert_eq!(account_meta.field_tag, "password");
    assert_eq!(account_meta.service, None);
    assert_eq!(account_meta.account, None);
    assert_eq!(AccountSecrets::SECURE_FIELDS.len(), 1);
    assert_eq!(
        AccountSecrets::SECURE_FIELDS[0].model_tag,
        account_meta.model_tag
    );
    assert_eq!(
        AccountSecrets::SECURE_FIELDS[0].field_tag,
        account_meta.field_tag
    );
    assert_eq!(
        AccountSecrets::SECURE_FIELDS[0].service,
        account_meta.service
    );
    assert_eq!(
        AccountSecrets::SECURE_FIELDS[0].account,
        account_meta.account
    );
    assert_eq!(account_meta.secure_fields.len(), 1);
    assert_eq!(account_meta.secure_fields[0].field_tag, "password");

    let override_key =
        <AppdbSensitiveFieldTagOverrideSecretsApiKey as SensitiveFieldTag>::crypto_metadata();
    let override_note =
        <AppdbSensitiveFieldTagOverrideSecretsNote as SensitiveFieldTag>::crypto_metadata();
    assert_eq!(override_key.service, Some("billing"));
    assert_eq!(override_key.account, Some("tenant-master"));
    assert_eq!(override_note.service, Some("billing"));
    assert_eq!(override_note.account, Some("tenant-note"));

    set_default_crypto_service("svc-b");
    set_default_crypto_account("acct-b");
    let mutated = default_crypto_config();
    assert_eq!(mutated.service, "svc-b");
    assert_eq!(mutated.account, "acct-b");
    reset_default_crypto_config();
}

#[test]
fn sensitive_nested_child_roundtrip_inherits_parent_runtime_context() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("nested-child");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }

    let parent = NestedSensitiveParent {
        alias: "parent-a".into(),
        child: NestedSensitiveChild {
            label: "child-a".into(),
            secret: "nested-secret".into(),
        },
    };

    let encrypted = parent
        .encrypt_with_runtime_resolver()
        .expect("nested child should encrypt");
    let decrypted = NestedSensitiveParent::decrypt_with_runtime_resolver(&encrypted)
        .expect("nested child should decrypt");

    assert_eq!(decrypted, parent);
    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_nested_option_roundtrip_preserves_some_and_none() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("nested-option");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }

    let some_parent = OptionalNestedSensitiveParent {
        alias: "parent-some".into(),
        child: Some(NestedSensitiveChild {
            label: "child-some".into(),
            secret: "some-secret".into(),
        }),
    };
    let none_parent = OptionalNestedSensitiveParent {
        alias: "parent-none".into(),
        child: None,
    };

    let encrypted_some = some_parent
        .encrypt_with_runtime_resolver()
        .expect("nested option some should encrypt");
    let encrypted_none = none_parent
        .encrypt_with_runtime_resolver()
        .expect("nested option none should encrypt");

    assert_eq!(
        OptionalNestedSensitiveParent::decrypt_with_runtime_resolver(&encrypted_some)
            .expect("nested option some should decrypt"),
        some_parent
    );
    assert_eq!(
        OptionalNestedSensitiveParent::decrypt_with_runtime_resolver(&encrypted_none)
            .expect("nested option none should decrypt"),
        none_parent
    );

    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_nested_override_sibling_contexts_do_not_cross_decrypt() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("nested-override-isolation");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }

    let parent = OverrideNestedSensitiveParent {
        alias: "parent".into(),
        left: OverrideNestedSensitiveLeaf {
            label: "left".into(),
            secret: "left-secret".into(),
        },
        right: OverrideNestedSensitiveLeaf {
            label: "right".into(),
            secret: "right-secret".into(),
        },
    };

    let encrypted = parent
        .encrypt_with_runtime_resolver()
        .expect("nested override parent should encrypt");

    let left_ctx = appdb::crypto::resolve_crypto_context_for::<
        AppdbSensitiveFieldTagOverrideNestedSensitiveParentLeft,
    >()
    .expect("left context should resolve");
    let right_ctx = appdb::crypto::resolve_crypto_context_for::<
        AppdbSensitiveFieldTagOverrideNestedSensitiveParentRight,
    >()
    .expect("right context should resolve");

    assert_eq!(
        OverrideNestedSensitiveLeaf::decrypt_with_context(&encrypted.left, &left_ctx)
            .expect("left leaf should decrypt with left context"),
        parent.left
    );
    assert_eq!(
        OverrideNestedSensitiveLeaf::decrypt_with_context(&encrypted.right, &right_ctx)
            .expect("right leaf should decrypt with right context"),
        parent.right
    );
    assert!(matches!(
        OverrideNestedSensitiveLeaf::decrypt_with_context(&encrypted.left, &right_ctx),
        Err(CryptoError::Decrypt)
    ));
    assert!(matches!(
        OverrideNestedSensitiveLeaf::decrypt_with_context(&encrypted.right, &left_ctx),
        Err(CryptoError::Decrypt)
    ));
    assert_eq!(
        OverrideNestedSensitiveParent::decrypt_with_runtime_resolver(&encrypted)
            .expect("runtime resolver should still decrypt the full parent"),
        parent
    );

    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}

#[test]
fn sensitive_enum_bearing_values_roundtrip_inside_secure_container_runtime_seam() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_crypto_context_registry();
    reset_default_crypto_config();
    let local_appdata = crypto_test_local_appdata("enum-runtime-seam");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &local_appdata);
    }

    let parent = RuntimeSeamParent {
        alias: "enum-parent".into(),
        payload: SensitiveValueOf::from(RuntimeSeamPayload {
            alias: "enum-payload".into(),
            status: RuntimeSeamStatus::Draft,
            optional_status: Some(RuntimeSeamStatus::Published),
            state: RuntimeSeamState::Published {
                version: 7,
                tags: vec!["release".into(), "stable".into()],
            },
        }),
    };

    let encrypted = parent
        .encrypt_with_runtime_resolver()
        .expect("enum-bearing payload should encrypt inside secure container");
    assert_ne!(
        encrypted.payload,
        serde_json::to_vec(&*parent.payload).expect("payload should serialize")
    );

    let decrypted = RuntimeSeamParent::decrypt_with_runtime_resolver(&encrypted)
        .expect("enum-bearing payload should decrypt from secure container");
    assert_eq!(decrypted, parent);

    unsafe {
        std::env::remove_var("LOCALAPPDATA");
    }
    let _ = std::fs::remove_dir_all(local_appdata);
    clear_crypto_context_registry();
    reset_default_crypto_config();
}
