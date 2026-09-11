use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use appdb::connection::{InitDbOptions, get_db, init_db_with_options, reset_db};
use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

const CHILD_MODE_ENV: &str = "APPDB_RUNTIME_PERSISTENCE_CHILD_MODE";
const CHILD_PATH_ENV: &str = "APPDB_RUNTIME_PERSISTENCE_CHILD_PATH";
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const SENTINEL_ID: &str = "process-restart-sentinel";
const SENTINEL_VALUE: &str = "persisted-across-process-boundary";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct RuntimePersistenceRow {
    id: Id,
    value: String,
}

#[test]
fn runtime_persistence_survives_process_restart() {
    let path = runtime_persistence_path();

    let write_status = run_child("write", &path);
    assert_child_success("write", &path, write_status);

    let read_status = run_child("read", &path);
    assert_child_success("read", &path, read_status);

    std::fs::remove_dir_all(&path).unwrap_or_else(|error| {
        panic!(
            "both child processes succeeded, but test storage cleanup failed for {}: {error}",
            path.display()
        )
    });
}

#[test]
#[ignore = "internal child process target for runtime_persistence_survives_process_restart"]
fn runtime_persistence_child() {
    let Ok(mode) = std::env::var(CHILD_MODE_ENV) else {
        return;
    };
    let path = PathBuf::from(std::env::var_os(CHILD_PATH_ENV).unwrap_or_else(|| {
        panic!("{CHILD_PATH_ENV} must be set when running the persistence child")
    }));

    // Each child runs only this test, so its hook also observes background task
    // panics without changing the panic hook used by other tests.
    let panic_count = Arc::new(AtomicUsize::new(0));
    let observed_panics = Arc::clone(&panic_count);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        observed_panics.fetch_add(1, Ordering::SeqCst);
        previous_hook(info);
    }));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("persistence child runtime should build");
    let result = runtime.block_on(async {
        init_db_with_options(path.clone(), InitDbOptions::local_app()).await?;

        let expected = RuntimePersistenceRow {
            id: Id::from(SENTINEL_ID),
            value: SENTINEL_VALUE.to_owned(),
        };

        match mode.as_str() {
            "write" => {
                let saved = RuntimePersistenceRow::save(expected.clone()).await?;
                anyhow::ensure!(saved == expected, "saved sentinel differs from input");

                let loaded = RuntimePersistenceRow::get(SENTINEL_ID).await?;
                anyhow::ensure!(
                    loaded == expected,
                    "same-process read differs from saved sentinel"
                );
            }
            "read" => {
                let loaded = RuntimePersistenceRow::get(SENTINEL_ID).await?;
                anyhow::ensure!(
                    loaded == expected,
                    "restarted-process read differs from sentinel"
                );
            }
            other => anyhow::bail!("unsupported persistence child mode `{other}`"),
        }

        Ok::<_, anyhow::Error>(get_db()?.as_ref().clone())
    });
    let stale_db = result.unwrap_or_else(|error| {
        reset_db();
        panic!(
            "persistence child mode `{mode}` failed for {}: {error}",
            path.display()
        )
    });
    reset_db();
    drop(stale_db);
    assert_eq!(
        panic_count.load(Ordering::SeqCst),
        0,
        "runtime tasks must not panic"
    );
}

fn run_child(mode: &str, path: &Path) -> ExitStatus {
    let executable = std::env::current_exe().expect("current persistence test executable");
    let mut child = Command::new(executable)
        .arg("--exact")
        .arg("runtime_persistence_child")
        .arg("--ignored")
        .arg("--nocapture")
        .env(CHILD_MODE_ENV, mode)
        .env(CHILD_PATH_ENV, path)
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "failed to spawn persistence child mode `{mode}` for {}: {error}",
                path.display()
            )
        });

    wait_for_child(mode, path, &mut child)
}

fn wait_for_child(mode: &str, path: &Path, child: &mut Child) -> ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap_or_else(|error| {
            panic!("failed waiting for persistence child `{mode}`: {error}")
        }) {
            return status;
        }

        if started.elapsed() >= CHILD_TIMEOUT {
            let _ = child.kill();
            let killed_status = child
                .wait()
                .unwrap_or_else(|error| panic!("failed reaping timed-out child `{mode}`: {error}"));
            panic!(
                "persistence child mode `{mode}` exceeded {:?} for {}; killed with status {killed_status:?}",
                CHILD_TIMEOUT,
                path.display()
            );
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_child_success(mode: &str, path: &Path, status: ExitStatus) {
    assert!(
        status.success(),
        "persistence child mode `{mode}` failed with {status:?}; storage remains at {} for inspection",
        path.display()
    );
}

fn runtime_persistence_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "appdb_runtime_persistence_{}_{}",
        std::process::id(),
        nanos
    ))
}
