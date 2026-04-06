use super::{DbRuntime, InitDbOptions, get_db, make_schema_ddl_idempotent, reinit_db, reset_db};
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
