use crate::error::DBError;
use crate::model::schema;
use anyhow::Result;
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, RwLock};
use std::thread;
use std::time::Duration;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb_core::cnf::{ConfigMap, NOTIFICATIONS_CHANNEL_SIZE};
use surrealdb_core::dbs::Capabilities;
use surrealdb_core::kvs::Datastore;
use surrealdb_core::options::EngineOptions;
use tokio_util::sync::CancellationToken;

/// Shared SurrealDB handle used by the runtime and global facade.
pub type DbHandle = Arc<Surreal<Db>>;

static DB: LazyLock<RwLock<Option<DbRuntime>>> = LazyLock::new(|| RwLock::new(None));

/// Disk sync policy for the embedded local datastore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalStorageSync {
    /// Leave flushing to the operating system.
    Never,
    /// Sync every committed transaction.
    Every,
    /// Flush periodically on the storage engine's background lifecycle.
    Interval(Duration),
}

impl std::fmt::Display for LocalStorageSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Never => f.write_str("never"),
            Self::Every => f.write_str("every"),
            Self::Interval(duration) => f.write_str(&format_duration_param(*duration)),
        }
    }
}

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
    /// Optional disk sync policy for the embedded local datastore.
    pub local_storage_sync: Option<LocalStorageSync>,
    /// Optional SurrealKV memtable size bound in bytes.
    pub surreal_kv_max_memtable_size: Option<usize>,
}

impl InitDbOptions {
    /// Uses the storage policy appdb recommends for local interactive apps.
    ///
    /// This profile keeps application crates from depending on storage-engine
    /// tuning details while still bounding local recovery work after repeated
    /// development or desktop-session writes.
    pub fn local_app() -> Self {
        Self::default()
            .versioned(false)
            .changefeed_gc_interval(Some(Duration::ZERO))
            .local_storage_sync(Some(LocalStorageSync::Every))
            .surreal_kv_max_memtable_size(Some(16 * 1024 * 1024))
    }

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

    /// Sets the embedded local datastore disk sync policy.
    pub fn local_storage_sync(mut self, sync: Option<LocalStorageSync>) -> Self {
        self.local_storage_sync = sync;
        self
    }

    /// Sets the SurrealKV memtable size bound in bytes.
    pub fn surreal_kv_max_memtable_size(mut self, size: Option<usize>) -> Self {
        self.surreal_kv_max_memtable_size = size;
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
        let old_runtime = {
            let mut db = DB
                .write()
                .expect("global database lock should not be poisoned");
            db.replace(self.clone())
        };
        drop(old_runtime);
    }
}

#[derive(Debug)]
struct DbWorker {
    db: Option<DbHandle>,
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

                let caller_db = db.clone();
                if ready_tx.send(Ok(caller_db)).is_err() {
                    return;
                }

                let _ = shutdown_rx.await;
                drop(db);
                tokio::task::yield_now().await;
            });
        });

        let db = ready_rx.recv().map_err(|err| {
            anyhow::anyhow!("database worker failed before initialization: {err}")
        })??;

        Ok(Self {
            db: Some(db),
            shutdown_tx: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    fn detached(db: DbHandle) -> Self {
        Self {
            db: Some(db),
            shutdown_tx: None,
            thread: None,
        }
    }

    fn handle(&self) -> DbHandle {
        self.db
            .as_ref()
            .expect("database worker handle should exist before shutdown")
            .clone()
    }
}

impl Drop for DbWorker {
    fn drop(&mut self) {
        self.db.take();
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
    let capabilities = Capabilities::default();
    let (notifications, builder) = if capabilities.allows_live_query_notifications() {
        let (send, recv) = surrealdb_core::channel::bounded(NOTIFICATIONS_CHANNEL_SIZE);
        (Some(recv), Datastore::builder().with_notify(send))
    } else {
        (None, Datastore::builder())
    };

    let datastore = builder
        .with_config(local_datastore_config(options))
        .with_query_timeout(options.query_timeout)
        .with_transaction_timeout(options.transaction_timeout)
        .with_capabilities(capabilities)
        .build_with_path(&local_storage_datastore_path(&path))
        .await?;

    datastore.check_version().await?;
    datastore.bootstrap().await?;

    Ok(Surreal::unstable_from_datastore(
        CancellationToken::new(),
        Arc::new(datastore),
        notifications,
        local_engine_options(options),
    )
    .await?)
}

fn local_storage_datastore_path(path: &Path) -> String {
    format!("surrealkv://{}", path.display())
}

fn local_datastore_config(options: &InitDbOptions) -> ConfigMap {
    let mut config =
        ConfigMap::empty().with_key_value("datastore_versioned", options.versioned.to_string());

    if let Some(retention) = options.version_retention {
        config = config.with_key_value("datastore_retention", format_duration_param(retention));
    }
    if let Some(sync) = options.local_storage_sync {
        config = config.with_key_value("datastore_sync", sync.to_string());
    }
    if let Some(size) = options.surreal_kv_max_memtable_size {
        config = config.with_key_value("surrealkv_max_memtable_size", size.to_string());
    }

    config
}

fn local_engine_options(options: &InitDbOptions) -> EngineOptions {
    let mut engine = EngineOptions::default();
    if let Some(interval) = options.changefeed_gc_interval {
        engine.changefeed_gc_interval = interval;
    }
    engine
}

fn format_duration_param(duration: Duration) -> String {
    if duration.is_zero() {
        return "0".to_string();
    }

    let micros = duration.as_micros();
    if micros < 1_000 {
        return format!("{micros}us");
    }

    let millis = duration.as_millis();
    if micros % 1_000 == 0 && millis < 1_000 {
        return format!("{millis}ms");
    }

    let seconds = duration.as_secs();
    if duration.subsec_nanos() == 0 {
        const MINUTE: u64 = 60;
        const HOUR: u64 = 60 * MINUTE;
        const DAY: u64 = 24 * HOUR;

        if seconds.is_multiple_of(DAY) {
            return format!("{}d", seconds / DAY);
        }
        if seconds.is_multiple_of(HOUR) {
            return format!("{}h", seconds / HOUR);
        }
        if seconds.is_multiple_of(MINUTE) {
            return format!("{}m", seconds / MINUTE);
        }
        return format!("{seconds}s");
    }

    format!("{millis}ms")
}

async fn apply_schema(db: &DbHandle) -> Result<()> {
    for item in inventory::iter::<schema::SchemaItem> {
        apply_schema_ddl(db, item.ddl).await?;
    }
    for item in inventory::iter::<schema::HnswSchemaItem> {
        apply_schema_ddl(db, &item.index.ddl()).await?;
    }
    Ok(())
}

async fn apply_schema_ddl(db: &DbHandle, ddl: &str) -> Result<()> {
    let ddl = make_schema_ddl_idempotent(ddl);
    let response = db.query(ddl.as_ref()).await?;
    if let Err(err) = response.check() {
        let message = err.to_string();
        let used_fallback_ddl = matches!(ddl, Cow::Borrowed(_));
        if !used_fallback_ddl || !is_schema_already_defined_error(&message) {
            return Err(DBError::QueryResponse(message).into());
        }
    }
    Ok(())
}

/// Opens a database and installs it as the global runtime.
pub async fn init_db(path: PathBuf) -> Result<()> {
    init_db_with_options(path, InitDbOptions::default()).await
}

/// Opens a database with the storage policy appdb recommends for local
/// interactive apps and installs it as the global runtime.
pub async fn init_local_app_db(path: PathBuf) -> Result<()> {
    init_db_with_options(path, InitDbOptions::local_app()).await
}

/// Clears the installed global database handle.
pub fn reset_db() {
    let old_runtime = {
        let mut db = DB
            .write()
            .expect("global database lock should not be poisoned");
        db.take()
    };
    drop(old_runtime);
}

/// Clears the installed global database handle and removes the database path.
///
/// This is intended for development reset flows. The current process must exit
/// after calling it so no stale cloned handle can write into a path being
/// deleted.
pub fn reset_db_and_remove_path(path: impl AsRef<Path>) -> Result<()> {
    reset_db();
    remove_optional_storage_artifact(path.as_ref())?;
    Ok(())
}

fn remove_optional_storage_artifact(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
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
    let old_runtime = {
        let mut db = DB
            .write()
            .expect("global database lock should not be poisoned");
        db.replace(runtime)
    };
    drop(old_runtime);
    Ok(())
}

/// Opens a database and replaces any previously installed global runtime.
pub async fn reinit_db(path: PathBuf) -> Result<()> {
    reinit_db_with_options(path, InitDbOptions::default()).await
}

/// Opens a database with the local interactive app policy and replaces any
/// previously installed global runtime.
pub async fn reinit_local_app_db(path: PathBuf) -> Result<()> {
    reinit_db_with_options(path, InitDbOptions::local_app()).await
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
#[path = "tests.rs"]
mod tests;
