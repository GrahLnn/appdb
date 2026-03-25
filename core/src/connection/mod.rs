use crate::error::DBError;
use crate::model::schema;
use anyhow::Result;
use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, RwLock};
use std::thread;
use std::time::Duration;
use surrealdb::Surreal;
use surrealdb::engine::local::{Db, SurrealKv};
use surrealdb::opt::Config;

/// Shared SurrealDB handle used by the runtime and global facade.
pub type DbHandle = Arc<Surreal<Db>>;

static DB: LazyLock<RwLock<Option<DbRuntime>>> = LazyLock::new(|| RwLock::new(None));

/// Options used when opening the embedded SurrealDB runtime.
#[derive(Debug, Clone, Default)]
pub struct InitDbOptions {
    /// Enables SurrealKV versioned storage.
    pub versioned: bool,
    /// Optional retention window used when versioning is enabled.
    pub version_retention: Option<Duration>,
    /// Optional per-query timeout.
    pub query_timeout: Option<Duration>,
    /// Optional per-transaction timeout.
    pub transaction_timeout: Option<Duration>,
    /// Optional changefeed garbage-collection interval.
    pub changefeed_gc_interval: Option<Duration>,
    /// Enables SurrealDB AST payload storage.
    pub ast_payload: bool,
}

impl InitDbOptions {
    /// Enables or disables versioned storage.
    pub fn versioned(mut self, enabled: bool) -> Self {
        self.versioned = enabled;
        self
    }

    /// Sets the retention window for versioned storage.
    pub fn version_retention(mut self, duration: Option<Duration>) -> Self {
        self.version_retention = duration;
        self
    }

    /// Sets the query timeout.
    pub fn query_timeout(mut self, duration: Option<Duration>) -> Self {
        self.query_timeout = duration;
        self
    }

    /// Sets the transaction timeout.
    pub fn transaction_timeout(mut self, duration: Option<Duration>) -> Self {
        self.transaction_timeout = duration;
        self
    }

    /// Sets the changefeed garbage-collection interval.
    pub fn changefeed_gc_interval(mut self, duration: Option<Duration>) -> Self {
        self.changefeed_gc_interval = duration;
        self
    }

    /// Enables or disables AST payload storage.
    pub fn ast_payload(mut self, enabled: bool) -> Self {
        self.ast_payload = enabled;
        self
    }
}

/// Owned database runtime that can be installed globally or passed around directly.
#[derive(Debug, Clone)]
pub struct DbRuntime {
    db: DbHandle,
    worker: Arc<DbWorker>,
}

impl DbRuntime {
    /// Opens a runtime with default options.
    pub async fn open(path: PathBuf) -> Result<Self> {
        Self::open_with_options(path, InitDbOptions::default()).await
    }

    /// Opens a schema-managed runtime with explicit options and applies every
    /// registered schema inventory item before the handle becomes available.
    ///
    /// This managed-open contract is intentionally separate from schemaless
    /// persistence semantics: callers that use `InitDbOptions::default()` still
    /// get automatic table bootstrap on first write, but that guarantee must not
    /// rely on schema side effects from this startup path.
    pub async fn open_with_options(path: PathBuf, options: InitDbOptions) -> Result<Self> {
        fs::create_dir_all(&path)?;
        let worker = Arc::new(DbWorker::spawn(path, options)?);
        let runtime = Self {
            db: worker.handle(),
            worker,
        };
        Ok(runtime)
    }

    /// Wraps an existing SurrealDB handle.
    pub fn from_handle(db: DbHandle) -> Self {
        Self {
            worker: Arc::new(DbWorker::detached(db.clone())),
            db,
        }
    }

    /// Returns a clone of the underlying database handle.
    pub fn handle(&self) -> DbHandle {
        let _ = &self.worker;
        self.db.clone()
    }

    /// Installs this runtime into the global singleton used by facade helpers.
    pub fn install_global(&self) -> Result<()> {
        let mut db = DB
            .write()
            .expect("global database lock should not be poisoned");
        if db.is_some() {
            return Err(DBError::AlreadyInitialized.into());
        }

        *db = Some(self.clone());
        Ok(())
    }

    #[doc(hidden)]
    pub fn reinstall_global_for_tests(&self) {
        let mut db = DB
            .write()
            .expect("global database lock should not be poisoned");
        *db = Some(self.clone());
    }
}

#[derive(Debug)]
struct DbWorker {
    db: DbHandle,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl DbWorker {
    fn spawn(path: PathBuf, options: InitDbOptions) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        let thread = thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(err.into()));
                    return;
                }
            };

            runtime.block_on(async move {
                let db = match open_db(path, &options).await {
                    Ok(db) => db,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                if let Err(err) = db.use_ns("app").use_db("app").await {
                    let _ = ready_tx.send(Err(err.into()));
                    return;
                }

                let db = Arc::new(db);
                if let Err(err) = apply_schema(&db).await {
                    let _ = ready_tx.send(Err(err));
                    return;
                }

                if ready_tx.send(Ok(db)).is_err() {
                    return;
                }

                let _ = shutdown_rx.await;
            });
        });

        let db = ready_rx.recv().map_err(|err| {
            anyhow::anyhow!("database worker failed before initialization: {err}")
        })??;

        Ok(Self {
            db,
            shutdown_tx: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    fn detached(db: DbHandle) -> Self {
        Self {
            db,
            shutdown_tx: None,
            thread: None,
        }
    }

    fn handle(&self) -> DbHandle {
        self.db.clone()
    }
}

impl Drop for DbWorker {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn is_schema_already_defined_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("already exists") || lower.contains("already defined")
}

fn make_schema_ddl_idempotent(ddl: &str) -> Cow<'_, str> {
    let trimmed = ddl.trim_start();
    if !trimmed
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("DEFINE"))
        || trimmed.to_ascii_uppercase().contains("IF NOT EXISTS")
    {
        return Cow::Borrowed(ddl);
    }

    for keyword in ["TABLE", "FIELD", "INDEX"] {
        let needle = format!("DEFINE {keyword}");
        if let Some(pos) = find_case_insensitive(trimmed, &needle) {
            let insert_at = pos + needle.len();
            let leading_ws_len = ddl.len() - trimmed.len();
            let absolute_insert_at = leading_ws_len + insert_at;
            let mut normalized = String::with_capacity(ddl.len() + " IF NOT EXISTS".len());
            normalized.push_str(&ddl[..absolute_insert_at]);
            normalized.push_str(" IF NOT EXISTS");
            normalized.push_str(&ddl[absolute_insert_at..]);
            return Cow::Owned(normalized);
        }
    }

    Cow::Borrowed(ddl)
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let haystack_upper = haystack.to_ascii_uppercase();
    let needle_upper = needle.to_ascii_uppercase();
    haystack_upper.find(&needle_upper)
}

async fn open_db(path: PathBuf, options: &InitDbOptions) -> Result<Surreal<Db>> {
    let config = Config::new()
        .set_ast_payload(options.ast_payload)
        .query_timeout(options.query_timeout)
        .transaction_timeout(options.transaction_timeout)
        .changefeed_gc_interval(options.changefeed_gc_interval);

    let mut builder = Surreal::new::<SurrealKv>((path, config));
    if options.versioned {
        builder = builder.versioned();
        if let Some(retention) = options.version_retention {
            builder = builder.retention(retention);
        }
    }

    Ok(builder.await?)
}

async fn apply_schema(db: &DbHandle) -> Result<()> {
    for item in inventory::iter::<schema::SchemaItem> {
        let ddl = make_schema_ddl_idempotent(item.ddl);
        let response = db.query(ddl.as_ref()).await?;
        if let Err(err) = response.check() {
            let message = err.to_string();
            let used_fallback_ddl = matches!(ddl, Cow::Borrowed(_));
            if !used_fallback_ddl || !is_schema_already_defined_error(&message) {
                return Err(DBError::QueryResponse(message).into());
            }
        }
    }
    Ok(())
}

/// Opens a database and installs it as the global runtime.
pub async fn init_db(path: PathBuf) -> Result<()> {
    init_db_with_options(path, InitDbOptions::default()).await
}

/// Clears the installed global database handle.
pub fn reset_db() {
    let mut db = DB
        .write()
        .expect("global database lock should not be poisoned");
    *db = None;
}

/// Opens a database with explicit options and installs it globally.
pub async fn init_db_with_options(path: PathBuf, options: InitDbOptions) -> Result<()> {
    let runtime = DbRuntime::open_with_options(path, options).await?;
    runtime.install_global()?;
    Ok(())
}

/// Opens a database with explicit options and replaces any previously installed global runtime.
pub async fn reinit_db_with_options(path: PathBuf, options: InitDbOptions) -> Result<()> {
    let runtime = DbRuntime::open_with_options(path, options).await?;
    let mut db = DB
        .write()
        .expect("global database lock should not be poisoned");
    *db = Some(runtime);
    Ok(())
}

/// Opens a database and replaces any previously installed global runtime.
pub async fn reinit_db(path: PathBuf) -> Result<()> {
    reinit_db_with_options(path, InitDbOptions::default()).await
}

/// Returns the global database handle previously installed by [`init_db`] or [`DbRuntime::install_global`].
pub fn get_db() -> Result<DbHandle> {
    DB.read()
        .expect("global database lock should not be poisoned")
        .as_ref()
        .map(DbRuntime::handle)
        .ok_or(DBError::NotInitialized.into())
}

#[cfg(test)]
mod tests {
    use super::{
        DbRuntime, InitDbOptions, get_db, make_schema_ddl_idempotent, reinit_db, reset_db,
    };
    use std::path::PathBuf;
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::Duration;
    use surrealdb::Surreal;

    static TEST_DB_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn default_init_options_are_non_versioned() {
        let options = InitDbOptions::default();
        assert!(!options.versioned);
        assert!(options.version_retention.is_none());
        assert!(options.query_timeout.is_none());
        assert!(options.transaction_timeout.is_none());
        assert!(options.changefeed_gc_interval.is_none());
        assert!(!options.ast_payload);
    }

    #[test]
    fn init_options_builders_override_values() {
        let options = InitDbOptions::default()
            .versioned(true)
            .version_retention(Some(Duration::from_secs(60)))
            .query_timeout(Some(Duration::from_secs(3)))
            .transaction_timeout(Some(Duration::from_secs(9)))
            .changefeed_gc_interval(Some(Duration::from_secs(30)))
            .ast_payload(true);

        assert!(options.versioned);
        assert_eq!(options.version_retention, Some(Duration::from_secs(60)));
        assert_eq!(options.query_timeout, Some(Duration::from_secs(3)));
        assert_eq!(options.transaction_timeout, Some(Duration::from_secs(9)));
        assert_eq!(
            options.changefeed_gc_interval,
            Some(Duration::from_secs(30))
        );
        assert!(options.ast_payload);
    }

    #[test]
    fn idempotent_schema_rewrites_define_table() {
        let ddl = "DEFINE TABLE user SCHEMAFULL;";
        assert_eq!(
            make_schema_ddl_idempotent(ddl),
            "DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;"
        );
    }

    #[test]
    fn idempotent_schema_rewrites_define_field() {
        let ddl = "DEFINE FIELD email ON user TYPE string;";
        assert_eq!(
            make_schema_ddl_idempotent(ddl),
            "DEFINE FIELD IF NOT EXISTS email ON user TYPE string;"
        );
    }

    #[test]
    fn idempotent_schema_rewrites_define_index() {
        let ddl = "DEFINE INDEX user_email ON user FIELDS email UNIQUE;";
        assert_eq!(
            make_schema_ddl_idempotent(ddl),
            "DEFINE INDEX IF NOT EXISTS user_email ON user FIELDS email UNIQUE;"
        );
    }

    #[test]
    fn idempotent_schema_preserves_existing_if_not_exists() {
        let ddl = "DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;";
        assert_eq!(make_schema_ddl_idempotent(ddl), ddl);
    }

    #[test]
    fn idempotent_schema_leaves_other_statements_unchanged() {
        let ddl = "REMOVE TABLE user;";
        assert_eq!(make_schema_ddl_idempotent(ddl), ddl);
    }

    #[test]
    fn runtime_wraps_existing_handle() {
        let handle = Arc::new(Surreal::init());
        let runtime = DbRuntime::from_handle(handle.clone());
        assert!(Arc::ptr_eq(&runtime.handle(), &handle));
    }

    #[test]
    fn reinstall_global_for_tests_replaces_existing_handle() {
        let _guard = TEST_DB_LOCK
            .lock()
            .expect("test db lock should not be poisoned");
        reset_db();

        let first = DbRuntime::from_handle(Arc::new(Surreal::init()));
        first.reinstall_global_for_tests();
        let initial = get_db().expect("db should be installed");
        assert!(Arc::ptr_eq(&initial, &first.handle()));

        let second = DbRuntime::from_handle(Arc::new(Surreal::init()));
        second.reinstall_global_for_tests();
        let reinstalled = get_db().expect("db should be reinstalled");
        assert!(Arc::ptr_eq(&reinstalled, &second.handle()));
        assert!(!Arc::ptr_eq(&reinstalled, &first.handle()));

        reset_db();
    }

    #[test]
    fn reset_db_clears_installed_handle() {
        let _guard = TEST_DB_LOCK
            .lock()
            .expect("test db lock should not be poisoned");
        reset_db();

        let runtime = DbRuntime::from_handle(Arc::new(Surreal::init()));
        runtime.reinstall_global_for_tests();

        reset_db();

        let err = get_db().expect_err("db should be reset");
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn reinit_db_survives_sequential_runtime_teardown() {
        let _guard = TEST_DB_LOCK
            .lock()
            .expect("test db lock should not be poisoned");
        reset_db();

        fn temp_path() -> PathBuf {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            std::env::temp_dir().join(format!(
                "appdb_connection_runtime_teardown_{}_{}",
                std::process::id(),
                nanos
            ))
        }

        fn open_and_reinstall(path: PathBuf) -> anyhow::Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;

            runtime.block_on(async {
                reinit_db(path).await?;
                let db = get_db()?;
                db.query("RETURN 1;").await?;
                Ok(())
            })
        }

        let first_path = temp_path();
        open_and_reinstall(first_path.clone()).expect("first runtime cycle should succeed");

        let stale = get_db().expect("first db should remain globally installed");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build")
            .block_on(async {
                stale
                    .query("RETURN 1;")
                    .await
                    .expect("stale handle should stay usable while its worker is still alive");
            });

        let second_path = temp_path();
        open_and_reinstall(second_path.clone()).expect("second runtime cycle should succeed");

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build")
            .block_on(async {
                let db = get_db().expect("second db should be installed");
                db.query("RETURN 2;")
                    .await
                    .expect("fresh handle should succeed on a new runtime");
            });

        reset_db();
        drop(stale);
        let _ = std::fs::remove_dir_all(first_path);
        let _ = std::fs::remove_dir_all(second_path);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn reinit_db_replaces_a_closed_runtime() {
        let _guard = TEST_DB_LOCK
            .lock()
            .expect("test db lock should not be poisoned");
        reset_db();

        fn temp_path() -> PathBuf {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            std::env::temp_dir().join(format!(
                "appdb_connection_reinit_{}_{}",
                std::process::id(),
                nanos
            ))
        }

        let first_path = temp_path();
        reinit_db(first_path.clone())
            .await
            .expect("first init should succeed");
        let first = get_db().expect("first db should be installed");
        first
            .query("RETURN 1;")
            .await
            .expect("first query should succeed");

        reset_db();
        drop(first);

        let second_path = temp_path();
        reinit_db(second_path.clone())
            .await
            .expect("second init should succeed");
        let second = get_db().expect("second db should be installed");
        second
            .query("RETURN 2;")
            .await
            .expect("second query should succeed");

        reset_db();
        let _ = std::fs::remove_dir_all(first_path);
        let _ = std::fs::remove_dir_all(second_path);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn repeated_reinit_keeps_cleanup_queries_usable() {
        let _guard = TEST_DB_LOCK
            .lock()
            .expect("test db lock should not be poisoned");
        reset_db();

        fn temp_path() -> PathBuf {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            std::env::temp_dir().join(format!(
                "appdb_connection_repeat_{}_{}",
                std::process::id(),
                nanos
            ))
        }

        let first_path = temp_path();
        reinit_db(first_path.clone())
            .await
            .expect("first init should succeed");
        let first = get_db().expect("first db should be installed");
        first
            .query("DEFINE TABLE temp_test; DELETE temp_test;")
            .await
            .expect("first cleanup should succeed");

        reset_db();
        drop(first);

        let second_path = temp_path();
        reinit_db(second_path.clone())
            .await
            .expect("second init should succeed");
        let second = get_db().expect("second db should be installed");
        second
            .query("DEFINE TABLE temp_test; DELETE temp_test;")
            .await
            .expect("second cleanup should succeed");
        second
            .query("RETURN 1;")
            .await
            .expect("second query should succeed");

        reset_db();
        let _ = std::fs::remove_dir_all(first_path);
        let _ = std::fs::remove_dir_all(second_path);
    }
}
