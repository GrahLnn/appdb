use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use appdb::connection::{DbRuntime, LocalStorageSync, init_local_app_db, reset_db_and_remove_path};
use appdb::model::meta::ModelMeta;
use appdb::repository::Repo;
use appdb::{Crud, Id, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use surrealdb::Surreal;
use surrealdb::engine::local::SurrealKv;
use surrealdb::types::{RecordId, SurrealValue, Table};
use tokio::runtime::Runtime;

static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static TEST_RT: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("performance runtime should be created")
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue, Store)]
struct PerfLoadRow {
    id: Id,
    #[pagin]
    sequence: i64,
    payload: String,
    version: i64,
}

#[derive(Debug, Clone, Copy)]
struct StorageStats {
    bytes: u64,
    files: u64,
}

#[derive(Debug, Clone, Copy)]
struct ReadTimings {
    page_ms: f64,
    get_ms: f64,
    list_limit_ms: f64,
    list_all_ms: f64,
    loaded_rows: usize,
}

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    TEST_RT.block_on(future)
}

fn temp_db_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "appdb_perf_load_{label}_{}_{}",
        std::process::id(),
        nanos
    ))
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn load_rows(prefix: &str, start: usize, count: usize, version: i64) -> Vec<PerfLoadRow> {
    let payload = "x".repeat(256);
    (start..start + count)
        .map(|sequence| PerfLoadRow {
            id: Id::from(format!("{prefix}-{sequence:08}")),
            sequence: sequence as i64,
            payload: payload.clone(),
            version,
        })
        .collect()
}

fn storage_stats(path: &Path) -> StorageStats {
    storage_stats_at_depth(path, 0)
}

fn storage_stats_at_depth(path: &Path, depth: usize) -> StorageStats {
    assert!(depth < 5, "storage measurement exceeds its maximum depth");
    let Ok(entries) = fs::read_dir(path) else {
        return StorageStats { bytes: 0, files: 0 };
    };

    entries
        .flatten()
        .fold(StorageStats { bytes: 0, files: 0 }, |mut total, entry| {
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => return total,
            };
            if metadata.is_dir() {
                let nested = storage_stats_at_depth(&entry.path(), depth + 1);
                total.bytes += nested.bytes;
                total.files += nested.files;
            } else {
                total.bytes += metadata.len();
                total.files += 1;
            }
            total
        })
}

fn ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

fn emit_metric(scenario: &str, fields: &[(&str, String)]) {
    let mut line = format!("APPDB_PERF {{\"scenario\":\"{scenario}\"");
    for (key, value) in fields {
        line.push_str(&format!(",\"{key}\":{value}"));
    }
    line.push('}');
    println!("{line}");
}

async fn open_database(path: PathBuf) -> Duration {
    let started = Instant::now();
    init_local_app_db(path)
        .await
        .expect("performance database should open");
    started.elapsed()
}

async fn seed_rows(prefix: &str, count: usize) -> Vec<PerfLoadRow> {
    let rows = load_rows(prefix, 0, count, 0);
    let inserted = PerfLoadRow::insert(rows.clone())
        .await
        .expect("seed insert should succeed");
    assert_eq!(inserted.len(), count);
    rows
}

async fn churn_rows(
    mut rows: Vec<PerfLoadRow>,
    delete_count: usize,
    rounds: usize,
) -> Vec<PerfLoadRow> {
    let mut next_sequence = rows.len();
    for round in 1..=rounds {
        let mut updated = rows.clone();
        for row in &mut updated {
            row.version = round as i64;
        }
        let saved = PerfLoadRow::save_many(updated)
            .await
            .expect("churn update should succeed");
        assert_eq!(saved.len(), rows.len());
        rows = saved;

        let removed = rows
            .drain(..delete_count.min(rows.len()))
            .collect::<Vec<_>>();
        for row in removed {
            Repo::<PerfLoadRow>::delete_record(RecordId::new(
                PerfLoadRow::table_name(),
                row.id.clone().into_record_id_key(),
            ))
            .await
            .expect("churn delete should succeed");
        }

        let inserted = load_rows("churn", next_sequence, delete_count, round as i64);
        next_sequence += delete_count;
        let created = PerfLoadRow::insert(inserted.clone())
            .await
            .expect("churn insert should succeed");
        assert_eq!(created.len(), inserted.len());
        rows.extend(inserted);
    }
    rows
}

async fn measure_reads(sample_id: &str, mut expected: Vec<PerfLoadRow>) -> ReadTimings {
    let started = Instant::now();
    let page = Repo::<PerfLoadRow>::pagin_asc(100, None)
        .await
        .expect("first page should load");
    let page_ms = ms(started.elapsed());
    assert!(!page.items.is_empty());

    let started = Instant::now();
    let loaded = PerfLoadRow::get(sample_id)
        .await
        .expect("sample row should load");
    let get_ms = ms(started.elapsed());
    assert_eq!(loaded.id, Id::from(sample_id));

    let started = Instant::now();
    let limited = PerfLoadRow::list_limit(100)
        .await
        .expect("bounded list should load");
    let list_limit_ms = ms(started.elapsed());
    assert_eq!(limited.len(), 100);

    let started = Instant::now();
    let mut all = PerfLoadRow::list().await.expect("full list should load");
    let list_all_ms = ms(started.elapsed());
    all.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    expected.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(
        all, expected,
        "all retained rows and payload versions must survive the process restart"
    );

    ReadTimings {
        page_ms,
        get_ms,
        list_limit_ms,
        list_all_ms,
        loaded_rows: all.len(),
    }
}

fn child_path() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDB_PERF_PATH").expect("APPDB_PERF_PATH must be set"))
}

async fn run_child_mode(mode: &str) {
    let path = child_path();
    let rows = env_usize("APPDB_PERF_ROWS", 20_000);
    let delete_count = env_usize("APPDB_PERF_DELETE_ROWS", rows / 4);
    let rounds = env_usize("APPDB_PERF_CHURN_ROUNDS", 3);
    if mode == "diagnose" {
        diagnose_open_and_pagination(&path).await;
        return;
    }
    let open_ms = ms(open_database(path.clone()).await);

    match mode {
        "seed" => {
            let started = Instant::now();
            let seeded = seed_rows("baseline", rows).await;
            assert_eq!(seeded.len(), rows);
            let write_ms = ms(started.elapsed());
            let stats = storage_stats(&path);
            emit_metric(
                "seed",
                &[
                    ("rows", rows.to_string()),
                    ("open_ms", format!("{open_ms:.3}")),
                    ("write_ms", format!("{write_ms:.3}")),
                    ("bytes", stats.bytes.to_string()),
                    ("files", stats.files.to_string()),
                ],
            );
        }
        "read_baseline" | "read_post_churn" => {
            let sample_id = if mode == "read_baseline" {
                format!("baseline-{last:08}", last = rows - 1)
            } else {
                format!("churn-{rows:08}")
            };
            let expected = if mode == "read_baseline" {
                load_rows("baseline", 0, rows, 0)
            } else {
                let removed = delete_count * rounds;
                let mut expected = load_rows("baseline", removed, rows - removed, rounds as i64);
                expected.extend(load_rows("churn", rows, removed, rounds as i64));
                expected
            };
            let reads = measure_reads(&sample_id, expected).await;
            assert_eq!(reads.loaded_rows, rows);
            let stats = storage_stats(&path);
            emit_metric(
                mode,
                &[
                    ("rows", rows.to_string()),
                    ("open_ms", format!("{open_ms:.3}")),
                    ("page_ms", format!("{:.3}", reads.page_ms)),
                    ("get_ms", format!("{:.3}", reads.get_ms)),
                    ("list_limit_ms", format!("{:.3}", reads.list_limit_ms)),
                    ("list_all_ms", format!("{:.3}", reads.list_all_ms)),
                    ("loaded_rows", reads.loaded_rows.to_string()),
                    ("bytes", stats.bytes.to_string()),
                    ("files", stats.files.to_string()),
                ],
            );
        }
        "churn" => {
            let started = Instant::now();
            let loaded = PerfLoadRow::list()
                .await
                .expect("churn source rows should load");
            assert_eq!(loaded.len(), rows);
            let load_all_ms = ms(started.elapsed());
            let started = Instant::now();
            let churned = churn_rows(loaded, delete_count, rounds).await;
            let churn_ms = ms(started.elapsed());
            assert_eq!(churned.len(), rows);
            let stats = storage_stats(&path);
            emit_metric(
                "churn",
                &[
                    ("rows", rows.to_string()),
                    ("delete_rows_per_round", delete_count.to_string()),
                    ("churn_rounds", rounds.to_string()),
                    ("open_ms", format!("{open_ms:.3}")),
                    ("load_all_ms", format!("{load_all_ms:.3}")),
                    ("churn_ms", format!("{churn_ms:.3}")),
                    ("bytes", stats.bytes.to_string()),
                    ("files", stats.files.to_string()),
                ],
            );
        }
        other => panic!("unknown APPDB_PERF_CHILD mode: {other}"),
    }
}

async fn diagnose_open_and_pagination(path: &Path) {
    let before = storage_stats(path);
    let started = Instant::now();
    let db = Arc::new(
        Surreal::new::<SurrealKv>(path.to_path_buf())
            .sync(LocalStorageSync::Every)
            .await
            .expect("raw public SDK engine should open"),
    );
    let engine_open_ms = ms(started.elapsed());
    let started = Instant::now();
    db.use_ns("app")
        .use_db("app")
        .await
        .expect("namespace selection should succeed");
    let select_namespace_ms = ms(started.elapsed());
    DbRuntime::from_handle(db.clone())
        .install_global()
        .expect("diagnostic handle should install");

    let started = Instant::now();
    let page = PerfLoadRow::pagin_asc(100, None)
        .await
        .expect("existing pagination should load");
    let materialized_page_ms = ms(started.elapsed());
    let started = Instant::now();
    let mut response = db
        .query("SELECT * FROM $table ORDER BY sequence ASC, id ASC LIMIT 100;")
        .bind(("table", Table::from(PerfLoadRow::table_name())))
        .await
        .expect("direct bounded query should run");
    response = response
        .check()
        .expect("direct bounded query should succeed");
    let direct: Vec<PerfLoadRow> = response.take(0).expect("direct rows should decode");
    let direct_page_ms = ms(started.elapsed());
    assert_eq!(
        page.items, direct,
        "direct query must preserve page contents and ordering"
    );
    let mut explain = db
        .query("SELECT * FROM $table ORDER BY sequence ASC, id ASC LIMIT 100 EXPLAIN FULL;")
        .bind(("table", Table::from(PerfLoadRow::table_name())))
        .await
        .expect("explain should run");
    let plan: surrealdb::types::Value = explain.take(0).expect("explain should decode");
    println!("APPDB_PERF_PLAN {plan:?}");
    let before_schema = storage_stats(path);

    let started = Instant::now();
    let mut ddl = inventory::iter::<appdb::model::schema::SchemaItem>
        .into_iter()
        .map(|item| item.ddl)
        .collect::<Vec<_>>();
    ddl.sort_unstable();
    for statement in ddl {
        db.query(statement)
            .await
            .expect("schema statement should execute")
            .check()
            .expect("schema statement should succeed");
    }
    let overwrite_schema_ms = ms(started.elapsed());
    let after_schema = storage_stats(path);
    emit_metric(
        "diagnose",
        &[
            ("engine_open_ms", format!("{engine_open_ms:.3}")),
            ("select_namespace_ms", format!("{select_namespace_ms:.3}")),
            ("materialized_page_ms", format!("{materialized_page_ms:.3}")),
            ("direct_page_ms", format!("{direct_page_ms:.3}")),
            ("overwrite_schema_ms", format!("{overwrite_schema_ms:.3}")),
            ("bytes_before_open", before.bytes.to_string()),
            ("bytes_before_schema", before_schema.bytes.to_string()),
            ("bytes_after_schema", after_schema.bytes.to_string()),
        ],
    );
}

fn spawn_phase(path: &Path, mode: &str) -> Value {
    println!("APPDB_PERF_PHASE {mode}");
    let output = Command::new(std::env::current_exe().expect("test executable should resolve"))
        .args([
            "--exact",
            "perf_cold_start_and_churned_reload",
            "--ignored",
            "--nocapture",
        ])
        .env("APPDB_PERF_CHILD", mode)
        .env("APPDB_PERF_PATH", path)
        .output()
        .expect("performance child process should start");
    assert!(
        output.status.success(),
        "performance child {mode} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.starts_with("APPDB_PERF_PLAN ") {
            println!("{line}");
        }
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("APPDB_PERF ")
                .and_then(|value| serde_json::from_str(value).ok())
        })
        .unwrap_or_else(|| {
            panic!(
                "performance child {mode} did not emit a metric\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
}

fn copy_metric_fields(target: &mut Map<String, Value>, prefix: &str, source: &Value) {
    let Some(object) = source.as_object() else {
        panic!("performance metric {prefix} must be an object");
    };
    for (key, value) in object {
        if key != "scenario" && key != "rows" {
            target.insert(format!("{prefix}_{key}"), value.clone());
        }
    }
}

fn run_scenario(label: &str, rows: usize, delete_count: usize, rounds: usize) {
    assert!(rows > 0, "APPDB_PERF_ROWS must be positive");
    assert!(delete_count > 0, "APPDB_PERF_DELETE_ROWS must be positive");
    assert!(
        delete_count <= rows,
        "delete count must not exceed row count"
    );
    assert!(
        delete_count
            .checked_mul(rounds)
            .is_some_and(|removed| removed <= rows),
        "this scenario deletes only original baseline rows; total deleted rows must not exceed the baseline"
    );
    let path = temp_db_path(label);

    let seed = spawn_phase(&path, "seed");
    let baseline = spawn_phase(&path, "read_baseline");
    let churn = spawn_phase(&path, "churn");
    let post_churn = spawn_phase(&path, "read_post_churn");
    let diagnose = std::env::var_os("APPDB_PERF_DIAGNOSE").map(|_| spawn_phase(&path, "diagnose"));

    let mut report = Map::new();
    report.insert("scenario".into(), Value::String("load_speed".into()));
    report.insert("rows".into(), json!(rows));
    report.insert("delete_rows_per_round".into(), json!(delete_count));
    report.insert("churn_rounds".into(), json!(rounds));
    copy_metric_fields(&mut report, "seed", &seed);
    copy_metric_fields(&mut report, "baseline", &baseline);
    copy_metric_fields(&mut report, "churn", &churn);
    copy_metric_fields(&mut report, "post_churn", &post_churn);
    if let Some(diagnose) = diagnose {
        copy_metric_fields(&mut report, "diagnose", &diagnose);
    }
    println!("APPDB_PERF {}", Value::Object(report));

    reset_db_and_remove_path(&path).expect("final performance database cleanup should succeed");
}

#[test]
#[ignore = "manual real-storage startup and churn performance test"]
fn perf_cold_start_and_churned_reload() {
    if let Ok(mode) = std::env::var("APPDB_PERF_CHILD") {
        run_async(run_child_mode(&mode));
        return;
    }

    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let rows = env_usize("APPDB_PERF_ROWS", 20_000);
    let delete_count = env_usize("APPDB_PERF_DELETE_ROWS", rows / 4);
    let rounds = env_usize("APPDB_PERF_CHURN_ROUNDS", 3);
    run_scenario("load", rows, delete_count, rounds);
}
