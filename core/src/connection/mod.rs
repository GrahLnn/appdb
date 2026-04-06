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
#[path = "tests.rs"]
mod tests;
