use super::*;
use std::sync::Mutex;

#[derive(Debug)]
struct MemorySecretStore {
    value: Mutex<Option<String>>,
}

impl MemorySecretStore {
    fn empty() -> Self {
        Self {
            value: Mutex::new(None),
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn read_secret(&self) -> Result<String, CryptoError> {
        self.value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(CryptoError::SecretNotFound)
    }

    fn write_secret(&self, value: &str) -> Result<(), CryptoError> {
        *self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value.to_owned());
        Ok(())
    }
}

#[derive(Debug)]
struct MemoryBackupStore {
    value: Mutex<Option<Vec<u8>>>,
}

impl MemoryBackupStore {
    fn empty() -> Self {
        Self {
            value: Mutex::new(None),
        }
    }
}

impl KeyBackupStore for MemoryBackupStore {
    fn read_key(&self) -> Result<Vec<u8>, CryptoError> {
        self.value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(CryptoError::SecretNotFound)
    }

    fn write_key(&self, value: &[u8]) -> Result<(), CryptoError> {
        *self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value.to_vec());
        Ok(())
    }
}

#[test]
fn keyring_provider_persists_generated_key() {
    let store = MemorySecretStore::empty();

    let first = load_or_generate_key(&store, None::<&MemoryBackupStore>)
        .expect("provider should generate key");
    let second = load_or_generate_key(&store, None::<&MemoryBackupStore>)
        .expect("provider should reuse key");

    assert_eq!(first.len(), KEY_LEN);
    assert_eq!(first, second);
}

#[test]
fn keyring_provider_restores_missing_secret_from_backup() {
    let store = MemorySecretStore::empty();
    let backup = MemoryBackupStore::empty();
    backup
        .write_key(&[7_u8; KEY_LEN])
        .expect("backup write should succeed");

    let key = load_or_generate_key(&store, Some(&backup))
        .expect("provider should restore key from backup");

    assert_eq!(key, vec![7_u8; KEY_LEN]);
    assert_eq!(
        store.read_secret().expect("store should be rewritten"),
        encode_hex(&key)
    );
}

#[test]
fn keyring_provider_mirrors_existing_secret_into_backup() {
    let store = MemorySecretStore::empty();
    let backup = MemoryBackupStore::empty();
    let key = vec![9_u8; KEY_LEN];
    store
        .write_secret(&encode_hex(&key))
        .expect("store write should succeed");

    let loaded = load_or_generate_key(&store, Some(&backup))
        .expect("provider should load existing store secret");

    assert_eq!(loaded, key);
    assert_eq!(
        backup.read_key().expect("backup should be populated"),
        loaded
    );
}

#[test]
fn crypto_context_registry_is_model_and_field_scoped() {
    struct ModelAField;
    struct ModelBField;

    impl SensitiveFieldTag for ModelAField {
        fn model_tag() -> &'static str {
            "model-a"
        }

        fn field_tag() -> &'static str {
            "secret"
        }
    }

    impl SensitiveFieldTag for ModelBField {
        fn model_tag() -> &'static str {
            "model-b"
        }

        fn field_tag() -> &'static str {
            "secret"
        }
    }

    clear_crypto_context_registry();
    let a = CryptoContext::new([1_u8; KEY_LEN]).expect("context should build");
    let b = CryptoContext::new([2_u8; KEY_LEN]).expect("context should build");

    register_crypto_context_for::<ModelAField>(a.clone());
    register_crypto_context_for::<ModelBField>(b.clone());

    let resolved_a =
        resolve_crypto_context_for::<ModelAField>().expect("model a mapping should resolve");
    let resolved_b =
        resolve_crypto_context_for::<ModelBField>().expect("model b mapping should resolve");

    let ciphertext = encrypt_string("value", &resolved_a).expect("encrypt should work");
    let err = decrypt_string(&ciphertext, &resolved_b).expect_err("wrong context should fail");

    assert_eq!(resolved_a.clone().key, a.key);
    assert_eq!(resolved_b.clone().key, b.key);
    assert!(matches!(err, CryptoError::Decrypt));
    clear_crypto_context_registry();
}
