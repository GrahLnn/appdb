use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use appdb::connection::{get_db, init_db, DbRuntime};
use appdb::graph::{GraphCrud, GraphRepo};
use appdb::model::meta::{HasId, register_table, ModelMeta};
use appdb::model::relation::relation_name;
use appdb::query::{query_bound_return, RawSqlStmt};
use appdb::repository::Repo;
use appdb::tx::{run_tx, TxStmt};
use appdb::{Crud, Id, Relation, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue, Table};
use tokio::runtime::Runtime;
use tokio::sync::OnceCell;

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static TEST_RT: LazyLock<Runtime> =
    LazyLock::new(|| Runtime::new().expect("integration runtime should be created"));

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItStringUser {
    id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItNumberUser {
    id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct ItRecordUser {
    id: RecordId,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItNoId {
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItProfile {
    id: Id,
    #[unique]
    name: String,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItCompositeUnique {
    id: Id,
    #[unique]
    name: String,
    #[unique]
    locale: String,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItFallbackLookup {
    name: String,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItLookupSource {
    #[unique]
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct ItLookupTarget {
    #[unique]
    code: String,
}

impl ModelMeta for ItRecordUser {
    fn table_name() -> &'static str {
        static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        TABLE_NAME.get_or_init(|| register_table(stringify!(ItRecordUser), "it_record_user"))
    }
}

impl Crud for ItRecordUser {}

impl HasId for ItRecordUser {
    fn id(&self) -> RecordId {
        self.id.clone()
    }
}

#[async_trait::async_trait]
impl appdb::model::meta::ResolveRecordId for ItRecordUser {
    async fn resolve_record_id(&self) -> anyhow::Result<RecordId> {
        Ok(self.id())
    }
}

#[derive(Debug, Clone, Copy, Relation)]
#[relation(name = "it_follows_rel")]
struct ItFollowsRel;

#[derive(Debug, Clone, Copy, Relation)]
struct AutoNamedTestRelation;

#[derive(Debug, Clone, Relation)]
struct NamedFieldTestRelation {
    created_at: i64,
}

fn test_db_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("appdb_it_{}_{}", std::process::id(), nanos))
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
    INIT.get_or_init(|| async {
        let path = test_db_path();
        init_db(path).await.expect("database should initialize");
    })
    .await;
}

#[test]
fn id_repo_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItStringUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        let inserted = Repo::<ItStringUser>::save(ItStringUser {
            id: Id::from("alice"),
        })
        .await
        .expect("save should succeed");
        assert_eq!(inserted.id, Id::from("alice"));

        let selected = Repo::<ItStringUser>::get("alice")
            .await
            .expect("get should succeed");
        assert_eq!(selected.id, Id::from("alice"));
    });
}

#[test]
fn inherent_model_api_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        ItStringUser::delete_all()
            .await
            .expect("delete_all should succeed");

        let inserted = Repo::<ItStringUser>::save(ItStringUser {
            id: Id::from("alice"),
        })
        .await
        .expect("save should succeed");

        let loaded = ItStringUser::get("alice")
            .await
            .expect("get should succeed");
        assert_eq!(loaded.id, Id::from("alice"));

        let listed = ItStringUser::list().await.expect("list should succeed");
        assert_eq!(listed.len(), 1);

        let ids = ItStringUser::list_record_ids()
            .await
            .expect("list_record_ids should succeed");
        assert_eq!(ids.len(), 1);

        let record = ItStringUser::list_record_ids()
            .await
            .expect("list_record_ids should succeed")
            .into_iter()
            .next()
            .expect("one record id should exist");

        assert_eq!(record.table, Table::from("it_string_user"));

        ItStringUser::delete("alice")
            .await
            .expect("delete should succeed");

        let err = ItStringUser::get("alice")
            .await
            .expect_err("deleted record should not load");
        assert!(err.to_string().contains("Record not found"));

        drop(inserted);
    });
}

#[test]
fn select_missing_record_fails() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        let _ = Repo::<ItStringUser>::save(ItStringUser {
            id: Id::from("seed"),
        })
        .await
        .expect("seed save should succeed");

        let err = Repo::<ItStringUser>::get("missing")
            .await
            .expect_err("missing record should fail");
        assert!(err.to_string().contains("Record not found"), "{err}");
    });
}

#[test]
fn number_id_repo_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItNumberUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        let inserted = Repo::<ItNumberUser>::save(ItNumberUser { id: Id::from(42i64) })
            .await
            .expect("save should succeed");
        assert_eq!(inserted.id, Id::from(42i64));

        let selected = Repo::<ItNumberUser>::list()
            .await
            .expect("list should succeed");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, Id::from(42i64));
    });
}

#[test]
fn db_runtime_opens_without_global_registration() {
    let _guard = acquire_test_lock();
    run_async(async {
        let runtime = DbRuntime::open(test_db_path())
            .await
            .expect("runtime should open");
        let db = runtime.handle();
        let mut result = db
            .query("RETURN 1;")
            .await
            .expect("query should succeed")
            .check()
            .expect("response should be valid");
        let value: Option<i64> = result.take(0).expect("result should decode");
        assert_eq!(value, Some(1));
    });
}

#[test]
fn save_preserves_payload_fields() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let inserted = Repo::<ItProfile>::save(ItProfile {
            id: Id::from("p1"),
            name: "alice".to_owned(),
            note: None,
        })
        .await
        .expect("save should succeed");

        assert_eq!(inserted.id, Id::from("p1"));
        assert_eq!(inserted.name, "alice");
        assert_eq!(inserted.note, None);

        let selected = Repo::<ItProfile>::get("p1")
            .await
            .expect("get should succeed");
        assert_eq!(selected.id, Id::from("p1"));
        assert_eq!(selected.name, "alice");
        assert_eq!(selected.note, None);
    });
}

#[test]
fn store_unique_field_registers_schema_index() {
    let ddls = inventory::iter::<appdb::model::schema::SchemaItem>
        .into_iter()
        .map(|item| item.ddl)
        .collect::<Vec<_>>();

    assert!(ddls.iter().any(|ddl| {
        ddl.contains("DEFINE INDEX IF NOT EXISTS it_profile_name_unique")
            && ddl.contains("ON it_profile")
            && ddl.contains("FIELDS name UNIQUE")
    }));
}

#[test]
fn store_auto_lookup_uses_unique_fields() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let saved = Repo::<ItProfile>::save(ItProfile {
            id: Id::from("p-lookup"),
            name: "alice".to_owned(),
            note: Some("x".to_owned()),
        })
        .await
        .expect("save should succeed");

        let id = Repo::<ItProfile>::find_unique_id_for(&ItProfile {
            id: Id::from("ignored"),
            name: "alice".to_owned(),
            note: None,
        })
        .await
        .expect("unique lookup should succeed");

        assert_eq!(id, saved.id());
    });
}

#[test]
fn store_auto_lookup_uses_all_unique_fields_together() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItCompositeUnique>::delete_all()
            .await
            .expect("delete_all should succeed");

        let saved = Repo::<ItCompositeUnique>::save(ItCompositeUnique {
            id: Id::from("cu-1"),
            name: "alice".to_owned(),
            locale: "en".to_owned(),
            note: Some("hello".to_owned()),
        })
        .await
        .expect("save should succeed");

        let id = Repo::<ItCompositeUnique>::find_unique_id_for(&ItCompositeUnique {
            id: Id::from("ignored"),
            name: "alice".to_owned(),
            locale: "en".to_owned(),
            note: None,
        })
        .await
        .expect("composite unique lookup should succeed");

        assert_eq!(id, saved.id());
    });
}

#[test]
fn store_auto_lookup_falls_back_to_non_id_fields() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItFallbackLookup>::delete_all()
            .await
            .expect("delete_all should succeed");

        let created = Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "fallback".to_owned(),
            note: Some("note".to_owned()),
        })
        .await
        .expect("create should succeed");

        let id = Repo::<ItFallbackLookup>::find_unique_id_for(&ItFallbackLookup {
            name: "fallback".to_owned(),
            note: Some("note".to_owned()),
        })
        .await
        .expect("fallback lookup should succeed");

        let loaded = Repo::<ItFallbackLookup>::get_record(id)
            .await
            .expect("get_record should succeed");
        assert_eq!(loaded.name, created.name);
        assert_eq!(loaded.note, created.note);
    });
}

#[test]
fn store_auto_lookup_errors_when_match_is_not_unique() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItFallbackLookup>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("first create should succeed");

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("second create should succeed");

        let err = Repo::<ItFallbackLookup>::find_unique_id_for(&ItFallbackLookup {
            name: "dup".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect_err("duplicate fallback lookup should fail");

        assert!(err.to_string().contains("multiple records"), "{err}");
    });
}

#[test]
fn store_auto_lookup_returns_not_found_when_unique_value_is_missing() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItProfile>::save(ItProfile {
            id: Id::from("present"),
            name: "alice".to_owned(),
            note: Some("x".to_owned()),
        })
        .await
        .expect("save should succeed");

        let err = Repo::<ItProfile>::find_unique_id_for(&ItProfile {
            id: Id::from("ignored"),
            name: "missing".to_owned(),
            note: None,
        })
        .await
        .expect_err("missing unique value should not resolve");

        assert!(err.to_string().contains("Record not found"), "{err}");
    });
}

#[test]
fn store_auto_lookup_requires_all_unique_fields_to_match() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItCompositeUnique>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItCompositeUnique>::save(ItCompositeUnique {
            id: Id::from("cu-present"),
            name: "alice".to_owned(),
            locale: "en".to_owned(),
            note: Some("hello".to_owned()),
        })
        .await
        .expect("save should succeed");

        let err = Repo::<ItCompositeUnique>::find_unique_id_for(&ItCompositeUnique {
            id: Id::from("ignored"),
            name: "alice".to_owned(),
            locale: "fr".to_owned(),
            note: None,
        })
        .await
        .expect_err("partial unique match should not resolve");

        assert!(err.to_string().contains("Record not found"), "{err}");
    });
}

#[test]
fn store_auto_lookup_with_missing_optional_field_does_not_match_unrelated_rows() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItFallbackLookup>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "null-case-a".to_owned(),
            note: None,
        })
        .await
        .expect("first create should succeed");

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "null-case-b".to_owned(),
            note: None,
        })
        .await
        .expect("second create should succeed");

        let err = Repo::<ItFallbackLookup>::find_unique_id_for(&ItFallbackLookup {
            name: "null-case-missing".to_owned(),
            note: None,
        })
        .await
        .expect_err("missing optional-field lookup should not false-positive");

        assert!(err.to_string().contains("Record not found"), "{err}");
    });
}

#[test]
fn save_many_batches_rows() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let inserted = Repo::<ItProfile>::save_many(vec![
            ItProfile {
                id: Id::from("p1"),
                name: "alice".to_owned(),
                note: Some("a".to_owned()),
            },
            ItProfile {
                id: Id::from("p2"),
                name: "bob".to_owned(),
                note: None,
            },
        ])
        .await
        .expect("batch save should succeed");

        assert_eq!(inserted.len(), 2);
        assert_eq!(inserted[0].id, Id::from("p1"));
        assert_eq!(inserted[0].name, "alice");
        assert_eq!(inserted[0].note.as_deref(), Some("a"));
        assert_eq!(inserted[1].id, Id::from("p2"));
        assert_eq!(inserted[1].name, "bob");
        assert_eq!(inserted[1].note, None);

        let selected = Repo::<ItProfile>::list()
            .await
            .expect("list should succeed");
        assert_eq!(selected.len(), 2);
    });
}

#[test]
fn upsert_id_without_id_field_fails() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        let err = Repo::<ItNoId>::save(ItNoId {
            name: "alice".to_owned(),
        })
        .await
        .expect_err("missing `id` field should fail");
        assert!(err
            .to_string()
            .contains("does not contain an `id` string or i64 field"));
    });
}

#[test]
fn graph_relation_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItRecordUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        let a = ItRecordUser {
            id: RecordId::new("it_record_user", "a"),
            name: "A".to_owned(),
        };
        let b = ItRecordUser {
            id: RecordId::new("it_record_user", "b"),
            name: "B".to_owned(),
        };

        Repo::<ItRecordUser>::create_at(a.id.clone(), a.clone())
            .await
            .expect("create a should succeed");
        Repo::<ItRecordUser>::create_at(b.id.clone(), b.clone())
            .await
            .expect("create b should succeed");

        let rel = relation_name::<ItFollowsRel>();
        GraphRepo::relate_at(a.id.clone(), b.id.clone(), rel)
            .await
            .expect("relate should succeed");

        let outs = GraphRepo::out_ids(a.id.clone(), rel, "it_record_user")
            .await
            .expect("out_ids should succeed");
        assert!(outs.iter().any(|id| id == &b.id));

        GraphRepo::unrelate_at(a.id.clone(), b.id.clone(), rel)
            .await
            .expect("unrelate should succeed");

        let outs_after = GraphRepo::out_ids(a.id.clone(), rel, "it_record_user")
            .await
            .expect("out_ids after unrelate should succeed");
        assert!(!outs_after.iter().any(|id| id == &b.id));
    });
}

#[test]
fn graph_instance_api_accepts_store_models_without_has_id() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");

        let source = Repo::<ItLookupSource>::create(ItLookupSource {
            name: "source-a".to_owned(),
        })
        .await
        .expect("source create should succeed");
        let target = Repo::<ItLookupTarget>::create(ItLookupTarget {
            code: "target-b".to_owned(),
        })
        .await
        .expect("target create should succeed");

        source
            .relate::<ItFollowsRel, _>(&target)
            .await
            .expect("instance relate should resolve record ids");

        let source_id = Repo::<ItLookupSource>::find_unique_id_for(&source)
            .await
            .expect("source lookup should succeed");
        let target_id = Repo::<ItLookupTarget>::find_unique_id_for(&target)
            .await
            .expect("target lookup should succeed");

        let outs = ItFollowsRel::out_ids(&source, "it_lookup_target")
            .await
            .expect("out_ids should succeed");
        assert!(outs.iter().any(|id| id == &target_id));

        source
            .unrelate::<ItFollowsRel, _>(&target)
            .await
            .expect("instance unrelate should resolve record ids");

        let outs_after = ItFollowsRel::out_ids(&source, "it_lookup_target")
            .await
            .expect("out_ids after unrelate should succeed");
        assert!(!outs_after.iter().any(|id| id == &target_id));

        let _ = source_id;
    });
}

#[test]
fn graph_instance_api_fails_when_source_store_lookup_is_ambiguous() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItFallbackLookup>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup-source".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("first source create should succeed");
        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup-source".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("second source create should succeed");

        let target = Repo::<ItLookupTarget>::create(ItLookupTarget {
            code: "ambiguous-target".to_owned(),
        })
        .await
        .expect("target create should succeed");

        let err = ItFallbackLookup {
            name: "dup-source".to_owned(),
            note: Some("same".to_owned()),
        }
        .relate::<ItFollowsRel, _>(&target)
        .await
        .expect_err("ambiguous source should fail graph relate");

        assert!(err.to_string().contains("multiple records"), "{err}");
    });
}

#[test]
fn graph_instance_api_fails_when_target_store_lookup_is_missing() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");

        let source = Repo::<ItLookupSource>::create(ItLookupSource {
            name: "good-source".to_owned(),
        })
        .await
        .expect("source create should succeed");

        let missing_target = ItLookupTarget {
            code: "missing-target".to_owned(),
        };

        let err = source
            .relate::<ItFollowsRel, _>(&missing_target)
            .await
            .expect_err("missing target should fail graph relate");

        assert!(err.to_string().contains("Record not found"), "{err}");
    });
}

#[test]
fn relation_out_ids_fails_when_store_input_is_ambiguous() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItFallbackLookup>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup-out".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("first create should succeed");
        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup-out".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("second create should succeed");

        let err = ItFollowsRel::out_ids(
            &ItFallbackLookup {
                name: "dup-out".to_owned(),
                note: Some("same".to_owned()),
            },
            "it_lookup_target",
        )
        .await
        .expect_err("ambiguous source should fail out_ids");

        assert!(err.to_string().contains("multiple records"), "{err}");
    });
}

#[test]
fn relation_type_api_accepts_store_models_without_has_id() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");

        let source = Repo::<ItLookupSource>::create(ItLookupSource {
            name: "source-type".to_owned(),
        })
        .await
        .expect("source create should succeed");
        let target = Repo::<ItLookupTarget>::create(ItLookupTarget {
            code: "target-type".to_owned(),
        })
        .await
        .expect("target create should succeed");

        ItFollowsRel::relate(&source, &target)
            .await
            .expect("type relate should resolve record ids");

        let target_id = Repo::<ItLookupTarget>::find_unique_id_for(&target)
            .await
            .expect("target lookup should succeed");

        let outs = ItFollowsRel::out_ids(&source, "it_lookup_target")
            .await
            .expect("type out_ids should succeed");
        assert!(outs.iter().any(|id| id == &target_id));
    });
}

#[test]
fn relation_derive_registers_default_and_overridden_names() {
    assert_eq!(relation_name::<ItFollowsRel>(), "it_follows_rel");
    assert_eq!(relation_name::<AutoNamedTestRelation>(), "auto_named_test_relation");
    assert_eq!(relation_name::<NamedFieldTestRelation>(), "named_field_test_relation");
}

#[test]
fn relation_type_api_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItRecordUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        let a = ItRecordUser {
            id: RecordId::new("it_record_user", "type-a"),
            name: "A".to_owned(),
        };
        let b = ItRecordUser {
            id: RecordId::new("it_record_user", "type-b"),
            name: "B".to_owned(),
        };

        Repo::<ItRecordUser>::create_at(a.id.clone(), a.clone())
            .await
            .expect("create a should succeed");
        Repo::<ItRecordUser>::create_at(b.id.clone(), b.clone())
            .await
            .expect("create b should succeed");

        ItFollowsRel::relate(&a, &b)
            .await
            .expect("type-level relate should succeed");

        let outs = ItFollowsRel::out_ids(&a, "it_record_user")
            .await
            .expect("type-level out_ids should succeed");
        assert!(outs.iter().any(|id| id == &b.id));

        ItFollowsRel::unrelate(&a, &b)
            .await
            .expect("type-level unrelate should succeed");

        let outs_after = ItFollowsRel::out_ids(&a, "it_record_user")
            .await
            .expect("type-level out_ids after unrelate should succeed");
        assert!(!outs_after.iter().any(|id| id == &b.id));
    });
}

#[test]
fn graph_instance_api_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItRecordUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        let a = ItRecordUser {
            id: RecordId::new("it_record_user", "inst-a"),
            name: "A".to_owned(),
        };
        let b = ItRecordUser {
            id: RecordId::new("it_record_user", "inst-b"),
            name: "B".to_owned(),
        };

        Repo::<ItRecordUser>::create_at(a.id.clone(), a.clone())
            .await
            .expect("create a should succeed");
        Repo::<ItRecordUser>::create_at(b.id.clone(), b.clone())
            .await
            .expect("create b should succeed");

        a.relate::<ItFollowsRel, _>(&b)
            .await
            .expect("instance relate should succeed");

        let outs = ItFollowsRel::out_ids(&a, "it_record_user")
            .await
            .expect("out_ids should succeed");
        assert!(outs.iter().any(|id| id == &b.id));

        a.unrelate::<ItFollowsRel, _>(&b)
            .await
            .expect("instance unrelate should succeed");

        let outs_after = ItFollowsRel::out_ids(&a, "it_record_user")
            .await
            .expect("out_ids after instance unrelate should succeed");
        assert!(!outs_after.iter().any(|id| id == &b.id));
    });
}

#[test]
fn inherent_record_model_api_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        ItRecordUser::delete_all()
            .await
            .expect("delete_all should succeed");

        let created = Repo::<ItRecordUser>::create_at(
            RecordId::new("it_record_user", "reload-me"),
            ItRecordUser {
                id: RecordId::new("it_record_user", "reload-me"),
                name: "before".to_owned(),
            },
        )
        .await
        .expect("create_at should succeed");

        let updated = Repo::<ItRecordUser>::update_at(
            RecordId::new("it_record_user", "reload-me"),
            ItRecordUser {
                id: RecordId::new("it_record_user", "reload-me"),
                name: "after".to_owned(),
            },
        )
        .await
        .expect("update_at should succeed");

        let reloaded = ItRecordUser::get_record(updated.id.clone())
            .await
            .expect("get_record should succeed");
        assert_eq!(reloaded.name, "after");

        Repo::<ItRecordUser>::delete_record(created.id.clone())
            .await
            .expect("record delete should succeed");
    });
}

#[test]
fn graph_relation_name_is_bound_as_identifier() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItRecordUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        let x = ItRecordUser {
            id: RecordId::new("it_record_user", "x"),
            name: "X".to_owned(),
        };
        let y = ItRecordUser {
            id: RecordId::new("it_record_user", "y"),
            name: "Y".to_owned(),
        };
        Repo::<ItRecordUser>::create_at(x.id.clone(), x.clone())
            .await
            .expect("create x should succeed");
        Repo::<ItRecordUser>::create_at(y.id.clone(), y.clone())
            .await
            .expect("create y should succeed");

        GraphRepo::relate_at(
            x.id.clone(),
            y.id.clone(),
            "bad-name; DELETE it_record_user RETURN NONE;",
        )
        .await
        .expect("relation name should be treated as bound identifier");

        let selected_x = Repo::<ItRecordUser>::get_record(RecordId::new("it_record_user", "x"))
            .await
            .expect("x should still exist");
        let selected_y = Repo::<ItRecordUser>::get_record(RecordId::new("it_record_user", "y"))
            .await
            .expect("y should still exist");
        assert_eq!(selected_x.name, "X");
        assert_eq!(selected_y.name, "Y");
    });
}

#[test]
fn delete_target_string_bind_fails_but_table_bind_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItRecordUser>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItRecordUser>::create_at(
            RecordId::new("it_record_user", "z"),
            ItRecordUser {
                id: RecordId::new("it_record_user", "z"),
                name: "Z".to_owned(),
            },
        )
        .await
        .expect("seed record should be created");

        let db = get_db().expect("db should be initialized");

        let bad_res = db
            .query("DELETE $target RETURN NONE;")
            .bind(("target", "it_record_user".to_owned()))
            .await
            .expect("query should execute");
        let bad_err = bad_res
            .check()
            .expect_err("string bind should fail for DELETE target");
        assert!(bad_err
            .to_string()
            .contains("Cannot execute DELETE statement using value"));

        db.query("DELETE $target RETURN NONE;")
            .bind(("target", Table::from("it_record_user")))
            .await
            .expect("query should execute")
            .check()
            .expect("table bind should pass for DELETE target");
    });
}

#[test]
fn transaction_runner_executes_and_returns_value() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        let stmt = TxStmt::new("RETURN $v;").bind("v", 42i64);
        let mut res = run_tx(vec![stmt]).await.expect("tx should succeed");
        let value: Option<i64> = res.take(0, 0).expect("take should decode value");
        assert_eq!(value, Some(42));
    });
}

#[test]
fn transaction_runner_returns_all_statement_results() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        let stmt1 = TxStmt::new("RETURN $v;").bind("v", 7i64);
        let stmt2 = TxStmt::new("RETURN $v;").bind("v", 11i64);
        let mut res = run_tx(vec![stmt1, stmt2]).await.expect("tx should succeed");

        assert_eq!(res.len(), 2);

        let first: Option<i64> = res.take(0, 0).expect("first statement should decode");
        let second: Option<i64> = res.take(1, 0).expect("second statement should decode");
        assert_eq!(first, Some(7));
        assert_eq!(second, Some(11));
    });
}

#[test]
fn raw_sql_stmt_binds_values_safely() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        let stmt = RawSqlStmt::new("RETURN $value;").bind("value", 99i64);
        let value = query_bound_return::<i64>(stmt)
            .await
            .expect("bound raw sql should succeed");
        assert_eq!(value, Some(99));
    });
}

