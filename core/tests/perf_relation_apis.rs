use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use appdb::connection::reinit_db;
use appdb::graph::GraphRepo;
use appdb::model::meta::ModelMeta;
use appdb::repository::Repo;
use appdb::{Id, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue};
use tokio::runtime::Runtime;

static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static TEST_RT: LazyLock<Runtime> =
    LazyLock::new(|| Runtime::new().expect("performance runtime should be created"));

const PERF_RELATE_ITEMS: &str = "perf_relate_items";
const PERF_BACK_RELATE_ITEMS: &str = "perf_back_relate_items";
const PERF_GRAPH_REL: &str = "perf_graph_rel";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue, Store)]
struct PerfRelationLeaf {
    id: Id,
    label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue, Store)]
struct PerfRelationRoot {
    id: Id,
    title: String,
    #[relate("perf_relate_items")]
    items: Vec<PerfRelationLeaf>,
    #[back_relate("perf_back_relate_items")]
    backlinks: Option<Vec<PerfRelationLeaf>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue, Store)]
struct PerfGraphNode {
    id: Id,
    label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue, Store)]
struct PerfPlainRow {
    id: Id,
    label: String,
    count: i64,
}

fn test_db_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "appdb_perf_relation_apis_{}_{}",
        std::process::id(),
        nanos
    ))
}

fn run_async<T>(fut: impl std::future::Future<Output = T>) -> T {
    TEST_RT.block_on(fut)
}

fn acquire_test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn ensure_db() {
    reinit_db(test_db_path())
        .await
        .expect("database should initialize");
}

fn ms_per_call(elapsed: Duration, iterations: u32) -> f64 {
    elapsed.as_secs_f64() * 1000.0 / f64::from(iterations)
}

fn emit_perf_metric(
    metric: &str,
    scenario: &str,
    iterations: u32,
    elapsed: Duration,
    dimensions: &[(&str, usize)],
) {
    let mut line = format!(
        "APPDB_PERF {{\"metric\":\"{metric}\",\"scenario\":\"{scenario}\",\"unit\":\"ms_per_call\",\"iterations\":{iterations},\"avg_ms\":{:.6}",
        ms_per_call(elapsed, iterations)
    );
    for (key, value) in dimensions {
        line.push_str(&format!(",\"{key}\":{value}"));
    }
    line.push('}');
    println!("{line}");
}

fn perf_leaf(id: String) -> PerfRelationLeaf {
    PerfRelationLeaf {
        id: Id::from(id.clone()),
        label: format!("label-{id}"),
    }
}

fn perf_root(id: &str, item_count: usize, backlink_count: usize) -> PerfRelationRoot {
    let items = (0..item_count)
        .map(|idx| perf_leaf(format!("{id}-item-{idx}")))
        .collect();
    let backlinks = Some(
        (0..backlink_count)
            .map(|idx| perf_leaf(format!("{id}-backlink-{idx}")))
            .collect(),
    );

    PerfRelationRoot {
        id: Id::from(id),
        title: format!("root-{id}"),
        items,
        backlinks,
    }
}

fn perf_root_batch(
    root_count: usize,
    item_count: usize,
    backlink_count: usize,
) -> Vec<PerfRelationRoot> {
    (0..root_count)
        .map(|idx| perf_root(&format!("batch-root-{idx}"), item_count, backlink_count))
        .collect()
}

fn perf_graph_node(id: String) -> PerfGraphNode {
    PerfGraphNode {
        id: Id::from(id.clone()),
        label: format!("node-{id}"),
    }
}

fn perf_plain_batch(prefix: &str, row_count: usize) -> Vec<PerfPlainRow> {
    (0..row_count)
        .map(|idx| PerfPlainRow {
            id: Id::from(format!("{prefix}-{idx}")),
            label: format!("plain-{prefix}-{idx}"),
            count: idx as i64,
        })
        .collect()
}

fn perf_graph_record(id: &str) -> RecordId {
    RecordId::new(PerfGraphNode::table_name(), id)
}

fn perf_root_record(id: &str) -> RecordId {
    RecordId::new(PerfRelationRoot::table_name(), id)
}

#[test]
#[ignore = "manual performance smoke test"]
fn perf_relation_field_save_smoke() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<PerfRelationRoot>::delete_all()
            .await
            .expect("root cleanup should succeed");
        Repo::<PerfRelationLeaf>::delete_all()
            .await
            .expect("leaf cleanup should succeed");

        let item_count = 64usize;
        let backlink_count = 64usize;
        let iterations = 8u32;
        let root = perf_root("single-root", item_count, backlink_count);

        let saved = PerfRelationRoot::save(root.clone())
            .await
            .expect("baseline save should succeed");
        assert_eq!(saved.items.len(), item_count);
        assert_eq!(saved.backlinks.as_ref().map(Vec::len), Some(backlink_count));

        let root_record = perf_root_record("single-root");
        assert_eq!(
            GraphRepo::outgoing_ids(root_record.clone(), PERF_RELATE_ITEMS)
                .await
                .expect("outgoing edge lookup should succeed")
                .len(),
            item_count
        );
        assert_eq!(
            GraphRepo::incoming_ids(root_record.clone(), PERF_BACK_RELATE_ITEMS)
                .await
                .expect("incoming edge lookup should succeed")
                .len(),
            backlink_count
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let saved = PerfRelationRoot::save(root.clone())
                .await
                .expect("repeated save should succeed");
            assert_eq!(saved.items.len(), item_count);
            assert_eq!(saved.backlinks.as_ref().map(Vec::len), Some(backlink_count));
        }
        let elapsed = start.elapsed();

        println!(
            "perf_relation_field_save_smoke: items={item_count}, backlinks={backlink_count}, avg_ms_per_save={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "relation_field_save",
            "single_root_replace_edges",
            iterations,
            elapsed,
            &[
                ("roots", 1),
                ("items_per_root", item_count),
                ("backlinks_per_root", backlink_count),
                ("edges_per_root", item_count + backlink_count),
            ],
        );
    });
}

#[test]
#[ignore = "manual performance smoke test"]
fn perf_relation_field_save_many_smoke() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<PerfRelationRoot>::delete_all()
            .await
            .expect("root cleanup should succeed");
        Repo::<PerfRelationLeaf>::delete_all()
            .await
            .expect("leaf cleanup should succeed");

        let root_count = 12usize;
        let item_count = 24usize;
        let backlink_count = 24usize;
        let iterations = 4u32;
        let batch = perf_root_batch(root_count, item_count, backlink_count);

        let saved = PerfRelationRoot::save_many(batch.clone())
            .await
            .expect("baseline save_many should succeed");
        assert_eq!(saved.len(), root_count);

        let sample_record = perf_root_record("batch-root-0");
        assert_eq!(
            GraphRepo::outgoing_ids(sample_record.clone(), PERF_RELATE_ITEMS)
                .await
                .expect("sample outgoing edge lookup should succeed")
                .len(),
            item_count
        );
        assert_eq!(
            GraphRepo::incoming_ids(sample_record.clone(), PERF_BACK_RELATE_ITEMS)
                .await
                .expect("sample incoming edge lookup should succeed")
                .len(),
            backlink_count
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let saved = PerfRelationRoot::save_many(batch.clone())
                .await
                .expect("repeated save_many should succeed");
            assert_eq!(saved.len(), root_count);
        }
        let elapsed = start.elapsed();

        println!(
            "perf_relation_field_save_many_smoke: roots={root_count}, items_per_root={item_count}, backlinks_per_root={backlink_count}, avg_ms_per_batch={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "relation_field_save_many",
            "batch_roots_replace_edges",
            iterations,
            elapsed,
            &[
                ("roots", root_count),
                ("items_per_root", item_count),
                ("backlinks_per_root", backlink_count),
                ("edges_per_root", item_count + backlink_count),
            ],
        );
    });
}

#[test]
#[ignore = "manual performance smoke test"]
fn perf_plain_store_write_paths_smoke() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<PerfPlainRow>::delete_all()
            .await
            .expect("plain row cleanup should succeed");

        let row_count = 512usize;
        let iterations = 4u32;

        let insert_batches: Vec<Vec<PerfPlainRow>> = (0..iterations)
            .map(|idx| perf_plain_batch(&format!("insert-{idx}"), row_count))
            .collect();
        let start = Instant::now();
        for batch in insert_batches {
            let inserted = Repo::<PerfPlainRow>::insert(batch)
                .await
                .expect("plain insert should succeed");
            assert_eq!(inserted.len(), row_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_plain_store_write_paths_smoke: insert rows={row_count}, avg_ms_per_batch={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "plain_store_write",
            "raw_insert_new_rows",
            iterations,
            elapsed,
            &[("rows", row_count)],
        );

        let ignore_seed = perf_plain_batch("insert-ignore-conflict", row_count);
        Repo::<PerfPlainRow>::insert(ignore_seed.clone())
            .await
            .expect("plain insert_ignore seed should succeed");
        let start = Instant::now();
        for _ in 0..iterations {
            let inserted = Repo::<PerfPlainRow>::insert_ignore(ignore_seed.clone())
                .await
                .expect("plain insert_ignore should succeed");
            assert!(inserted.is_empty());
        }
        let elapsed = start.elapsed();
        println!(
            "perf_plain_store_write_paths_smoke: insert_ignore all_conflicts rows={row_count}, avg_ms_per_batch={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "plain_store_write",
            "raw_insert_ignore_all_conflicts",
            iterations,
            elapsed,
            &[("rows", row_count)],
        );

        let replace_seed = perf_plain_batch("insert-or-replace-existing", row_count);
        Repo::<PerfPlainRow>::insert(replace_seed.clone())
            .await
            .expect("plain insert_or_replace seed should succeed");
        let start = Instant::now();
        for _ in 0..iterations {
            let replaced = Repo::<PerfPlainRow>::insert_or_replace(replace_seed.clone())
                .await
                .expect("plain insert_or_replace should succeed");
            assert_eq!(replaced.len(), row_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_plain_store_write_paths_smoke: insert_or_replace existing rows={row_count}, avg_ms_per_batch={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "plain_store_write",
            "raw_insert_or_replace_existing_rows",
            iterations,
            elapsed,
            &[("rows", row_count)],
        );

        let save_many_batch = perf_plain_batch("save-many-existing", row_count);
        PerfPlainRow::save_many(save_many_batch.clone())
            .await
            .expect("plain save_many seed should succeed");
        let start = Instant::now();
        for _ in 0..iterations {
            let saved = PerfPlainRow::save_many(save_many_batch.clone())
                .await
                .expect("plain save_many should succeed");
            assert_eq!(saved.len(), row_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_plain_store_write_paths_smoke: save_many existing rows={row_count}, avg_ms_per_batch={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "plain_store_write",
            "save_many_existing_rows",
            iterations,
            elapsed,
            &[("rows", row_count)],
        );
    });
}

#[test]
#[ignore = "manual performance smoke test"]
fn perf_store_graph_accessors_smoke() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<PerfGraphNode>::delete_all()
            .await
            .expect("node cleanup should succeed");

        let edge_count = 96usize;
        let iterations = 20u32;

        let mut nodes = Vec::with_capacity(1 + edge_count * 2);
        nodes.push(perf_graph_node("hub".to_owned()));
        for idx in 0..edge_count {
            nodes.push(perf_graph_node(format!("out-{idx}")));
            nodes.push(perf_graph_node(format!("in-{idx}")));
        }
        PerfGraphNode::save_many(nodes)
            .await
            .expect("node setup should succeed");

        let hub_record = perf_graph_record("hub");
        for idx in 0..edge_count {
            GraphRepo::relate_at(
                hub_record.clone(),
                perf_graph_record(&format!("out-{idx}")),
                PERF_GRAPH_REL,
            )
            .await
            .expect("hub outgoing relate should succeed");
            GraphRepo::relate_at(
                perf_graph_record(&format!("in-{idx}")),
                hub_record.clone(),
                PERF_GRAPH_REL,
            )
            .await
            .expect("hub incoming relate should succeed");
        }

        let hub = PerfGraphNode {
            id: Id::from("hub"),
            label: "node-hub".to_owned(),
        };

        assert_eq!(
            hub.outgoing_ids(PERF_GRAPH_REL)
                .await
                .expect("warmup outgoing_ids should succeed")
                .len(),
            edge_count
        );
        assert_eq!(
            hub.incoming_ids(PERF_GRAPH_REL)
                .await
                .expect("warmup incoming_ids should succeed")
                .len(),
            edge_count
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let ids = hub
                .outgoing_ids(PERF_GRAPH_REL)
                .await
                .expect("outgoing_ids should succeed");
            assert_eq!(ids.len(), edge_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: outgoing_ids avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "outgoing_ids",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let rows = hub
                .outgoing::<PerfGraphNode>(PERF_GRAPH_REL)
                .await
                .expect("outgoing rows should succeed");
            assert_eq!(rows.len(), edge_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: outgoing_rows avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "outgoing_rows",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let count = hub
                .outgoing_count(PERF_GRAPH_REL)
                .await
                .expect("outgoing_count should succeed");
            assert_eq!(count, edge_count as i64);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: outgoing_count avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "outgoing_count",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let count = hub
                .outgoing_count_as::<PerfGraphNode>(PERF_GRAPH_REL)
                .await
                .expect("typed outgoing_count should succeed");
            assert_eq!(count, edge_count as i64);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: outgoing_count_as avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "outgoing_count_as",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let ids = hub
                .incoming_ids(PERF_GRAPH_REL)
                .await
                .expect("incoming_ids should succeed");
            assert_eq!(ids.len(), edge_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: incoming_ids avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "incoming_ids",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let rows = hub
                .incoming::<PerfGraphNode>(PERF_GRAPH_REL)
                .await
                .expect("incoming rows should succeed");
            assert_eq!(rows.len(), edge_count);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: incoming_rows avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "incoming_rows",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let count = hub
                .incoming_count(PERF_GRAPH_REL)
                .await
                .expect("incoming_count should succeed");
            assert_eq!(count, edge_count as i64);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: incoming_count avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "incoming_count",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );

        let start = Instant::now();
        for _ in 0..iterations {
            let count = hub
                .incoming_count_as::<PerfGraphNode>(PERF_GRAPH_REL)
                .await
                .expect("typed incoming_count should succeed");
            assert_eq!(count, edge_count as i64);
        }
        let elapsed = start.elapsed();
        println!(
            "perf_store_graph_accessors_smoke: incoming_count_as avg_ms={:.3}",
            ms_per_call(elapsed, iterations)
        );
        emit_perf_metric(
            "graph_accessor",
            "incoming_count_as",
            iterations,
            elapsed,
            &[("edges", edge_count)],
        );
    });
}
