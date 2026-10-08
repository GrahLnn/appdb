use crate::error::DBError;
use crate::model::schema;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;
use surrealdb::Surreal;
use surrealdb::engine::local::{Db, SurrealKv};
use surrealdb::opt::Config;
use surrealdb::opt::capabilities::Capabilities;

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
///
/// Every option maps to the public SurrealDB SDK connection builder. Engine
/// internals are deliberately not part of this API.
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
    /// Enables SurrealDB AST payload storage.
    pub ast_payload: bool,
    /// Optional disk sync policy for the embedded local datastore.
    pub local_storage_sync: Option<LocalStorageSync>,
}

impl InitDbOptions {
    /// Uses the storage policy appdb recommends for local interactive apps.
    pub fn local_app() -> Self {
        Self::default()
            .versioned(false)
            .local_storage_sync(Some(LocalStorageSync::Every))
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

    /// Enables or disables AST payloads.
    pub fn ast_payload(mut self, enabled: bool) -> Self {
        self.ast_payload = enabled;
        self
    }

    /// Sets the embedded local datastore disk sync policy.
    pub fn local_storage_sync(mut self, sync: Option<LocalStorageSync>) -> Self {
        self.local_storage_sync = sync;
        self
    }
}

/// Owned database runtime that can be installed globally or passed around directly.
///
/// The runtime owns only the public SDK handle. Dropping the last handle lets
/// SurrealDB release its engine tasks without a synchronous worker join.
#[derive(Debug, Clone)]
pub struct DbRuntime {
    db: DbHandle,
}

impl DbRuntime {
    /// Opens a runtime with default options.
    pub async fn open(path: PathBuf) -> Result<Self> {
        Self::open_with_options(path, InitDbOptions::default()).await
    }

    /// Opens a schema-managed runtime with explicit options and applies the
    /// deterministic schema plan before the handle becomes available.
    pub async fn open_with_options(path: PathBuf, options: InitDbOptions) -> Result<Self> {
        fs::create_dir_all(&path)?;
        let schema_marker_path = path.join(".appdb-schema-fingerprint");

        let config = Config::new()
            .set_ast_payload(options.ast_payload)
            .query_timeout(options.query_timeout)
            .transaction_timeout(options.transaction_timeout)
            .capabilities(Capabilities::default());

        let mut connection = Surreal::new::<SurrealKv>((path, config));
        if options.versioned {
            connection = connection.versioned();
            if let Some(retention) = options.version_retention {
                connection = connection.retention(retention);
            }
        }
        if let Some(sync) = options.local_storage_sync {
            connection = connection.sync(sync);
        }
        let db = Arc::new(connection.await?);

        db.use_ns("app").use_db("app").await?;
        apply_schema(&db, &schema_marker_path).await?;

        Ok(Self { db })
    }

    /// Wraps an existing SurrealDB handle.
    pub fn from_handle(db: DbHandle) -> Self {
        Self { db }
    }

    /// Returns a clone of the underlying database handle.
    pub fn handle(&self) -> DbHandle {
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

async fn apply_schema(db: &DbHandle, marker_path: &Path) -> Result<()> {
    let mut ddl: Vec<String> = inventory::iter::<schema::SchemaItem>
        .into_iter()
        .map(|item| item.ddl.to_owned())
        .collect();
    ddl.extend(
        inventory::iter::<schema::HnswSchemaItem>
            .into_iter()
            .map(|item| item.index.ddl()),
    );
    ddl.sort_unstable();

    let fingerprint = schema::fingerprint(&ddl);
    if fs::read_to_string(marker_path)
        .ok()
        .is_some_and(|value| value == fingerprint)
    {
        return Ok(());
    }

    for statement in ddl {
        apply_schema_ddl(db, &statement).await?;
    }

    fs::write(marker_path, fingerprint)?;
    Ok(())
}

async fn apply_schema_ddl(db: &DbHandle, ddl: &str) -> Result<()> {
    let response = db.query(ddl).await?;
    response
        .check()
        .map_err(|err| DBError::QueryResponse(err.to_string()))?;
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
    let mut db = DB
        .write()
        .expect("global database lock should not be poisoned");
    db.take();
}

/// Clears the installed global database handle and removes the database path.
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
    let mut db = DB
        .write()
        .expect("global database lock should not be poisoned");
    db.replace(runtime);
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
