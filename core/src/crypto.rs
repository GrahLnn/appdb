use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use keyring::{Entry, Error as KeyringError};
use rand::RngExt;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use thiserror::Error;
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{LocalFree, HLOCAL};
#[cfg(target_os = "windows")]
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[derive(Debug, Error)]
/// Errors produced by key loading and field encryption helpers.
pub enum CryptoError {
    #[error("crypto key must be exactly {KEY_LEN} bytes")]
    InvalidKeyLength,
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("ciphertext is shorter than the required nonce length")]
    CiphertextTooShort,
    #[error("decrypted data is not valid UTF-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("stored key is not valid hex")]
    InvalidStoredKey,
    #[error("secret store entry not found")]
    SecretNotFound,
    #[error("secret store error: {0}")]
    SecretStore(String),
    #[error("protected backup store error: {0}")]
    ProtectedBackup(String),
    #[error("no crypto resolver mapping registered for model tag `{model_tag}` and field tag `{field_tag}`")]
    ResolverNotFound {
        model_tag: &'static str,
        field_tag: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stable generated tags used to resolve crypto contexts at runtime.
pub struct CryptoTag {
    pub model: &'static str,
    pub field: &'static str,
}

impl CryptoTag {
    /// Creates a model/field tag pair.
    pub const fn new(model: &'static str, field: &'static str) -> Self {
        Self { model, field }
    }
}

/// Generated metadata for a `Sensitive` plaintext/encrypted pair.
pub trait SensitiveModelTag {
    /// Stable tag identifying this sensitive model.
    fn model_tag() -> &'static str;
}

/// Generated metadata for a secure field on a `Sensitive` model.
pub trait SensitiveFieldTag {
    /// Stable model tag for the owning sensitive type.
    fn model_tag() -> &'static str;

    /// Stable field tag for the secure field.
    fn field_tag() -> &'static str;

    /// Generated crypto metadata for this secure field.
    fn crypto_metadata() -> &'static SensitiveFieldMetadata {
        static DEFAULT: SensitiveFieldMetadata = SensitiveFieldMetadata {
            model_tag: "",
            field_tag: "",
            service: None,
            account: None,
            secure_fields: &[],
        };
        &DEFAULT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Stable generated metadata for one secure field and its crypto configuration surface.
pub struct SensitiveFieldMetadata {
    pub model_tag: &'static str,
    pub field_tag: &'static str,
    pub service: Option<&'static str>,
    pub account: Option<&'static str>,
    pub secure_fields: &'static [SensitiveFieldMetadata],
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Process-wide default crypto strategy used by generated sensitive metadata.
pub struct DefaultCryptoConfig {
    pub service: String,
    pub account: String,
}

const DEFAULT_CRYPTO_SERVICE: &str = "appdb";
const DEFAULT_CRYPTO_ACCOUNT: &str = "master-sensitive";

static DEFAULT_CRYPTO_CONFIG: LazyLock<RwLock<DefaultCryptoConfig>> = LazyLock::new(|| {
    RwLock::new(DefaultCryptoConfig {
        service: DEFAULT_CRYPTO_SERVICE.to_owned(),
        account: DEFAULT_CRYPTO_ACCOUNT.to_owned(),
    })
});

/// Returns the current process-wide default crypto service/account pair.
pub fn default_crypto_config() -> DefaultCryptoConfig {
    DEFAULT_CRYPTO_CONFIG
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Replaces the process-wide default crypto service/account pair.
pub fn set_default_crypto_config(service: impl Into<String>, account: impl Into<String>) {
    *DEFAULT_CRYPTO_CONFIG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = DefaultCryptoConfig {
        service: service.into(),
        account: account.into(),
    };
}

/// Sets only the default crypto service, preserving the current default account.
pub fn set_default_crypto_service(service: impl Into<String>) {
    DEFAULT_CRYPTO_CONFIG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .service = service.into();
}

/// Sets only the default crypto account, preserving the current default service.
pub fn set_default_crypto_account(account: impl Into<String>) {
    DEFAULT_CRYPTO_CONFIG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .account = account.into();
}

/// Resets process-wide crypto defaults back to built-in values.
pub fn reset_default_crypto_config() {
    set_default_crypto_config(DEFAULT_CRYPTO_SERVICE, DEFAULT_CRYPTO_ACCOUNT);
}

type ResolverKey = (&'static str, &'static str);
type CryptoResolverRegistry = HashMap<ResolverKey, Arc<CryptoContext>>;
#[derive(Debug, Default)]
struct AutoCryptoInit {
    state: Mutex<AutoCryptoInitState>,
    ready: Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AutoCryptoInitState {
    #[default]
    Pending,
    Running,
    Ready,
}

type AutoCryptoRegistry = HashMap<&'static str, Arc<AutoCryptoInit>>;

static CRYPTO_RESOLVER_REGISTRY: LazyLock<RwLock<CryptoResolverRegistry>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static AUTO_CRYPTO_REGISTRY: LazyLock<RwLock<AutoCryptoRegistry>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Registers a crypto context for the generated tag of one secure field.
pub fn register_crypto_context_for<Tag>(context: CryptoContext)
where
    Tag: SensitiveFieldTag,
{
    register_crypto_context(CryptoTag::new(Tag::model_tag(), Tag::field_tag()), context);
}

/// Registers a crypto context for a concrete model/field tag pair.
pub fn register_crypto_context(tag: CryptoTag, context: CryptoContext) {
    CRYPTO_RESOLVER_REGISTRY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert((tag.model, tag.field), Arc::new(context));
}

/// Removes all registered crypto contexts. Intended for test isolation.
pub fn clear_crypto_context_registry() {
    CRYPTO_RESOLVER_REGISTRY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    AUTO_CRYPTO_REGISTRY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// Resolves a previously registered crypto context by generated field tag.
pub fn resolve_crypto_context_for<Tag>() -> Result<Arc<CryptoContext>, CryptoError>
where
    Tag: SensitiveFieldTag,
{
    ensure_sensitive_model_ready::<Tag>()?;
    resolve_crypto_context(CryptoTag::new(Tag::model_tag(), Tag::field_tag()))
}

/// Resolves a previously registered crypto context by explicit model/field tag pair.
pub fn resolve_crypto_context(tag: CryptoTag) -> Result<Arc<CryptoContext>, CryptoError> {
    CRYPTO_RESOLVER_REGISTRY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&(tag.model, tag.field))
        .cloned()
        .ok_or(CryptoError::ResolverNotFound {
            model_tag: tag.model,
            field_tag: tag.field,
        })
}

fn ensure_sensitive_model_ready<Tag>() -> Result<(), CryptoError>
where
    Tag: SensitiveFieldTag,
{
    let model_tag = Tag::model_tag();
    if CRYPTO_RESOLVER_REGISTRY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(&(model_tag, Tag::field_tag()))
    {
        return Ok(());
    }

    let once = {
        let mut registry = AUTO_CRYPTO_REGISTRY
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .entry(model_tag)
            .or_insert_with(|| Arc::new(AutoCryptoInit::default()))
            .clone()
    };

    once.run(register_model_crypto_fields::<Tag>)?;
    Ok(())
}

impl AutoCryptoInit {
    fn run(
        &self,
        init: impl FnOnce() -> Result<(), CryptoError>,
    ) -> Result<(), CryptoError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match *state {
                AutoCryptoInitState::Ready => return Ok(()),
                AutoCryptoInitState::Pending => {
                    *state = AutoCryptoInitState::Running;
                    drop(state);
                    let result = init();
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *state = if result.is_ok() {
                        AutoCryptoInitState::Ready
                    } else {
                        AutoCryptoInitState::Pending
                    };
                    self.ready.notify_all();
                    return result;
                }
                AutoCryptoInitState::Running => {
                    state = self
                        .ready
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
        }
    }
}

fn register_model_crypto_fields<Tag>() -> Result<(), CryptoError>
where
    Tag: SensitiveFieldTag,
{
    for meta in Tag::crypto_metadata().secure_fields {
        let context = Arc::new(build_context_for_metadata(meta)?);
        CRYPTO_RESOLVER_REGISTRY
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry((meta.model_tag, meta.field_tag))
            .or_insert(context);
    }
    Ok(())
}

fn build_context_for_metadata(meta: &SensitiveFieldMetadata) -> Result<CryptoContext, CryptoError> {
    let defaults = default_crypto_config();
    let service = meta.service.unwrap_or(defaults.service.as_str());
    let account = meta.account.unwrap_or(defaults.account.as_str());
    let provider = KeyringKeyProvider::new(service, account)?;
    CryptoContext::from_provider(&provider)
}

/// Source of a symmetric encryption key for [`CryptoContext`].
pub trait KeyProvider {
    /// Loads a raw 32-byte key.
    fn load_key(&self) -> Result<Vec<u8>, CryptoError>;
}

trait SecretStore {
    fn read_secret(&self) -> Result<String, CryptoError>;
    fn write_secret(&self, value: &str) -> Result<(), CryptoError>;
}

trait KeyBackupStore {
    fn read_key(&self) -> Result<Vec<u8>, CryptoError>;
    fn write_key(&self, value: &[u8]) -> Result<(), CryptoError>;
}

#[derive(Debug, Clone)]
/// In-memory key provider for tests or externally managed keys.
pub struct StaticKeyProvider {
    key: Vec<u8>,
}

impl StaticKeyProvider {
    /// Creates a provider from raw key bytes.
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { key: key.into() }
    }
}

impl KeyProvider for StaticKeyProvider {
    fn load_key(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(self.key.clone())
    }
}

#[derive(Debug)]
struct KeyringSecretStore {
    entry: Entry,
}

impl KeyringSecretStore {
    fn new(service: &str, account: &str) -> Result<Self, CryptoError> {
        let entry = Entry::new(service, account)
            .map_err(|error| CryptoError::SecretStore(error.to_string()))?;
        Ok(Self { entry })
    }
}

impl SecretStore for KeyringSecretStore {
    fn read_secret(&self) -> Result<String, CryptoError> {
        self.entry.get_password().map_err(map_keyring_error)
    }

    fn write_secret(&self, value: &str) -> Result<(), CryptoError> {
        self.entry
            .set_password(value)
            .map_err(|error| CryptoError::SecretStore(error.to_string()))
    }
}

#[derive(Debug)]
/// Key provider backed by the OS keyring with a protected local backup.
pub struct KeyringKeyProvider {
    store: KeyringSecretStore,
    backup: Option<DpapiKeyBackupStore>,
}

impl KeyringKeyProvider {
    /// Creates a keyring-backed provider for the given service and account.
    pub fn new(service: &str, account: &str) -> Result<Self, CryptoError> {
        Ok(Self {
            store: KeyringSecretStore::new(service, account)?,
            backup: DpapiKeyBackupStore::new(service, account),
        })
    }
}

impl KeyProvider for KeyringKeyProvider {
    fn load_key(&self) -> Result<Vec<u8>, CryptoError> {
        load_or_generate_key(&self.store, self.backup.as_ref())
    }
}

#[derive(Debug, Clone)]
/// Encryption context holding the symmetric key used by generated `Sensitive` types.
pub struct CryptoContext {
    key: [u8; KEY_LEN],
}

impl CryptoContext {
    /// Builds a context from raw key bytes.
    pub fn new(key: impl AsRef<[u8]>) -> Result<Self, CryptoError> {
        let key = key.as_ref();
        if key.len() != KEY_LEN {
            return Err(CryptoError::InvalidKeyLength);
        }

        let mut bytes = [0_u8; KEY_LEN];
        bytes.copy_from_slice(key);
        Ok(Self { key: bytes })
    }

    /// Builds a context by loading the key from a provider.
    pub fn from_provider(provider: &impl KeyProvider) -> Result<Self, CryptoError> {
        Self::new(provider.load_key()?)
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.key).expect("key length already validated")
    }
}

/// Encrypts arbitrary bytes and prefixes the output with a random nonce.
pub fn encrypt_bytes(value: &[u8], context: &CryptoContext) -> Result<Vec<u8>, CryptoError> {
    let cipher = context.cipher();
    let mut nonce_bytes = [0_u8; NONCE_LEN];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, value)
        .map_err(|_| CryptoError::Encrypt)?;

    let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypts bytes produced by [`encrypt_bytes`].
pub fn decrypt_bytes(value: &[u8], context: &CryptoContext) -> Result<Vec<u8>, CryptoError> {
    if value.len() < NONCE_LEN {
        return Err(CryptoError::CiphertextTooShort);
    }

    let (nonce_bytes, ciphertext) = value.split_at(NONCE_LEN);
    let cipher = context.cipher();
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

/// Encrypts a UTF-8 string into nonce-prefixed ciphertext bytes.
pub fn encrypt_string(value: &str, context: &CryptoContext) -> Result<Vec<u8>, CryptoError> {
    encrypt_bytes(value.as_bytes(), context)
}

/// Decrypts bytes from [`encrypt_string`] back into a UTF-8 string.
pub fn decrypt_string(value: &[u8], context: &CryptoContext) -> Result<String, CryptoError> {
    Ok(String::from_utf8(decrypt_bytes(value, context)?)?)
}

/// Encrypts an optional string while preserving `None`.
pub fn encrypt_optional_string(
    value: &Option<String>,
    context: &CryptoContext,
) -> Result<Option<Vec<u8>>, CryptoError> {
    value
        .as_ref()
        .map(|value| encrypt_string(value, context))
        .transpose()
}

/// Decrypts an optional encrypted string while preserving `None`.
pub fn decrypt_optional_string(
    value: &Option<Vec<u8>>,
    context: &CryptoContext,
) -> Result<Option<String>, CryptoError> {
    value
        .as_ref()
        .map(|value| decrypt_string(value, context))
        .transpose()
}

fn map_keyring_error(error: KeyringError) -> CryptoError {
    match error {
        KeyringError::NoEntry => CryptoError::SecretNotFound,
        other => CryptoError::SecretStore(other.to_string()),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(LUT[(byte >> 4) as usize] as char);
        output.push(LUT[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, CryptoError> {
    if value.len() != KEY_LEN * 2 {
        return Err(CryptoError::InvalidStoredKey);
    }

    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let hi = decode_hex_nibble(chunk[0])?;
            let lo = decode_hex_nibble(chunk[1])?;
            Ok((hi << 4) | lo)
        })
        .collect()
}

fn decode_hex_nibble(byte: u8) -> Result<u8, CryptoError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CryptoError::InvalidStoredKey),
    }
}

fn load_or_generate_key(
    store: &impl SecretStore,
    backup: Option<&impl KeyBackupStore>,
) -> Result<Vec<u8>, CryptoError> {
    match store.read_secret() {
        Ok(value) => {
            let key = decode_hex(&value)?;
            mirror_key_to_backup(&key, backup);
            Ok(key)
        }
        Err(CryptoError::SecretNotFound) => {
            if let Some(backup) = backup {
                match backup.read_key() {
                    Ok(key) => {
                        if key.len() != KEY_LEN {
                            return Err(CryptoError::InvalidKeyLength);
                        }
                        store.write_secret(&encode_hex(&key))?;
                        return Ok(key);
                    }
                    Err(CryptoError::SecretNotFound) => {}
                    Err(error) => return Err(error),
                }
            }

            let mut key = [0_u8; KEY_LEN];
            rand::rng().fill(&mut key);
            let encoded = encode_hex(&key);
            store.write_secret(&encoded)?;
            mirror_key_to_backup(&key, backup);
            Ok(key.to_vec())
        }
        Err(error) => Err(error),
    }
}

fn mirror_key_to_backup(key: &[u8], backup: Option<&impl KeyBackupStore>) {
    if let Some(backup) = backup {
        let _ = backup.write_key(key);
    }
}

#[derive(Debug, Clone)]
struct DpapiKeyBackupStore {
    path: PathBuf,
}

impl DpapiKeyBackupStore {
    fn new(service: &str, account: &str) -> Option<Self> {
        fallback_key_path(service, account).map(|path| Self { path })
    }
}

impl KeyBackupStore for DpapiKeyBackupStore {
    fn read_key(&self) -> Result<Vec<u8>, CryptoError> {
        if !self.path.exists() {
            return Err(CryptoError::SecretNotFound);
        }

        let bytes = fs::read(&self.path)
            .map_err(|error| CryptoError::ProtectedBackup(error.to_string()))?;
        unprotect_backup_bytes(&bytes)
    }

    fn write_key(&self, value: &[u8]) -> Result<(), CryptoError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| CryptoError::ProtectedBackup(error.to_string()))?;
        }

        let protected = protect_backup_bytes(value)?;
        fs::write(&self.path, protected)
            .map_err(|error| CryptoError::ProtectedBackup(error.to_string()))
    }
}

fn fallback_key_path(service: &str, account: &str) -> Option<PathBuf> {
    let root = env::var_os("LOCALAPPDATA")?;
    let service = sanitize_path_component(service);
    let account = sanitize_path_component(account);
    Some(
        Path::new(&root)
            .join(service)
            .join("key-backup")
            .join(format!("{account}.bin")),
    )
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn protect_backup_bytes(value: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: value.len() as u32,
        pbData: value.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| CryptoError::ProtectedBackup(error.to_string()))?;

    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn unprotect_backup_bytes(value: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: value.len() as u32,
        pbData: value.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|error| CryptoError::ProtectedBackup(error.to_string()))?;

    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
    }
    Ok(bytes)
}

#[cfg(not(target_os = "windows"))]
fn protect_backup_bytes(value: &[u8]) -> Result<Vec<u8>, CryptoError> {
    Ok(value.to_vec())
}

#[cfg(not(target_os = "windows"))]
fn unprotect_backup_bytes(value: &[u8]) -> Result<Vec<u8>, CryptoError> {
    Ok(value.to_vec())
}

#[cfg(test)]
mod tests {
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
}
