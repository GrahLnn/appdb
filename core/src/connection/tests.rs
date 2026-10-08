use super::{
    DbRuntime, InitDbOptions, LocalStorageSync, format_duration_param, get_db, reinit_db, reset_db,
    reset_db_and_remove_path,
};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;

static TEST_DB_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[test]
fn default_init_options_are_public_sdk_only() {
    let options = InitDbOptions::default();
    assert!(!options.versioned);
    assert!(options.version_retention.is_none());
    assert!(options.query_timeout.is_none());
    assert!(options.transaction_timeout.is_none());
    assert!(!options.ast_payload);
    assert!(options.local_storage_sync.is_none());
}

#[test]
fn init_options_builders_override_values() {
    let options = InitDbOptions::default()
        .versioned(true)
        .version_retention(Some(Duration::from_secs(60)))
        .query_timeout(Some(Duration::from_secs(3)))
        .transaction_timeout(Some(Duration::from_secs(9)))
        .ast_payload(true)
        .local_storage_sync(Some(LocalStorageSync::Interval(Duration::from_millis(200))));

    assert!(options.versioned);
    assert_eq!(options.version_retention, Some(Duration::from_secs(60)));
    assert_eq!(options.query_timeout, Some(Duration::from_secs(3)));
    assert_eq!(options.transaction_timeout, Some(Duration::from_secs(9)));
    assert!(options.ast_payload);
    assert_eq!(
        options.local_storage_sync,
        Some(LocalStorageSync::Interval(Duration::from_millis(200)))
    );
}

#[test]
fn local_app_init_options_own_interactive_storage_policy() {
    let options = InitDbOptions::local_app();
    assert!(!options.versioned);
    assert_eq!(options.local_storage_sync, Some(LocalStorageSync::Every));
}

#[test]
fn local_storage_sync_formats_for_surrealdb_endpoint_params() {
    assert_eq!(LocalStorageSync::Never.to_string(), "never");
    assert_eq!(LocalStorageSync::Every.to_string(), "every");
    assert_eq!(
        LocalStorageSync::Interval(Duration::from_millis(200)).to_string(),
        "200ms"
    );
    assert_eq!(
        LocalStorageSync::Interval(Duration::from_secs(1)).to_string(),
        "1s"
    );
}

#[test]
fn duration_params_use_surrealdb_duration_units() {
    assert_eq!(format_duration_param(Duration::ZERO), "0");
    assert_eq!(format_duration_param(Duration::from_micros(250)), "250us");
    assert_eq!(format_duration_param(Duration::from_millis(200)), "200ms");
    assert_eq!(format_duration_param(Duration::from_secs(1)), "1s");
    assert_eq!(format_duration_param(Duration::from_secs(60)), "1m");
}

#[test]
fn runtime_wraps_existing_public_sdk_handle() {
    let handle = Arc::new(Surreal::<Db>::init());
    let runtime = DbRuntime::from_handle(handle.clone());
    assert!(Arc::ptr_eq(&runtime.handle(), &handle));
}

#[test]
fn reinstall_global_for_tests_replaces_existing_handle() {
    let _guard = TEST_DB_LOCK
        .lock()
        .expect("test db lock should not be poisoned");
    reset_db();

    let first = DbRuntime::from_handle(Arc::new(Surreal::<Db>::init()));
    first.reinstall_global_for_tests();
    let initial = get_db().expect("db should be installed");
    assert!(Arc::ptr_eq(&initial, &first.handle()));

    let second = DbRuntime::from_handle(Arc::new(Surreal::<Db>::init()));
    second.reinstall_global_for_tests();
    let reinstalled = get_db().expect("db should be reinstalled");
    assert!(Arc::ptr_eq(&reinstalled, &second.handle()));
    assert!(!Arc::ptr_eq(&reinstalled, &first.handle()));

    reset_db();
}

#[tokio::test]
async fn public_sdk_runtime_opens_and_selects() {
    let path = temp_path("appdb_connection_public_sdk");
    let runtime = DbRuntime::open(path.clone())
        .await
        .expect("public SDK runtime should open");
    runtime
        .handle()
        .query("RETURN 1;")
        .await
        .expect("public SDK query should succeed");
    drop(runtime);
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn reinit_replaces_global_handle() {
    let _guard = TEST_DB_LOCK
        .lock()
        .expect("test db lock should not be poisoned");
    reset_db();
    let path = temp_path("appdb_connection_reinit_public_sdk");
    reinit_db(path.clone())
        .await
        .expect("reinit should install a public SDK runtime");
    get_db().expect("global runtime should exist");
    reset_db_and_remove_path(&path).expect("runtime path should be removable");
}

#[test]
fn reset_db_and_remove_path_removes_storage_artifact() {
    let _guard = TEST_DB_LOCK
        .lock()
        .expect("test db lock should not be poisoned");
    reset_db();

    let path = temp_path("appdb_connection_remove_artifact");
    let storage_artifact_dir = path.join("storage-artifacts");
    std::fs::create_dir_all(&storage_artifact_dir).expect("storage artifact should be created");
    std::fs::write(storage_artifact_dir.join("entry.bin"), b"test")
        .expect("storage file should be created");

    reset_db_and_remove_path(&path).expect("storage artifact should be removed");
    assert!(!path.exists());
}

fn temp_path(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), nanos))
}
