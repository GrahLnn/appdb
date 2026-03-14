use crate::error::DBError;
use crate::model::schema;
use anyhow::Result;
use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use surrealdb::engine::local::{Db, SurrealKv};
use surrealdb::opt::Config;
use surrealdb::Surreal;
use tokio::sync::OnceCell;

pub type DbHandle = Arc<Surreal<Db>>;

static DB: LazyLock<OnceCell<DbHandle>> = LazyLock::new(OnceCell::new);

#[derive(Debug, Clone)]
pub struct InitDbOptions {
    pub versioned: bool,
    pub version_retention: Option<Duration>,
    pub query_timeout: Option<Duration>,
    pub transaction_timeout: Option<Duration>,
    pub changefeed_gc_interval: Option<Duration>,
    pub ast_payload: bool,
}

impl Default for InitDbOptions {
    fn default() -> Self {
        Self {
            versioned: false,
            version_retention: None,
            query_timeout: None,
            transaction_timeout: None,
            changefeed_gc_interval: None,
            ast_payload: false,
        }
    }
}

impl InitDbOptions {
    pub fn versioned(mut self, enabled: bool) -> Self {
        self.versioned = enabled;
        self
    }

    pub fn version_retention(mut self, duration: Option<Duration>) -> Self {
        self.version_retention = duration;
        self
    }

    pub fn query_timeout(mut self, duration: Option<Duration>) -> Self {
        self.query_timeout = duration;
        self
    }

    pub fn transaction_timeout(mut self, duration: Option<Duration>) -> Self {
        self.transaction_timeout = duration;
        self
    }

    pub fn changefeed_gc_interval(mut self, duration: Option<Duration>) -> Self {
        self.changefeed_gc_interval = duration;
        self
    }

    pub fn ast_payload(mut self, enabled: bool) -> Self {
        self.ast_payload = enabled;
        self
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

pub async fn init_db(path: PathBuf) -> Result<()> {
    init_db_with_options(path, InitDbOptions::default()).await
}

pub async fn init_db_with_options(path: PathBuf, options: InitDbOptions) -> Result<()> {
    fs::create_dir_all(&path)?;
    let db = open_db(path, &options).await?;
    db.use_ns("app").use_db("app").await?;

    DB.set(Arc::new(db))
        .map_err(|_| DBError::AlreadyInitialized)?;
    let db = get_db()?;
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

pub fn get_db() -> Result<DbHandle> {
    DB.get().cloned().ok_or(DBError::NotInitialized.into())
}

#[cfg(test)]
mod tests {
    use super::{make_schema_ddl_idempotent, InitDbOptions};
    use std::time::Duration;

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
}
