use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use appdb::connection::{get_db, reinit_db, DbRuntime};
use appdb::crypto::{
    clear_crypto_context_registry, register_crypto_context_for, CryptoContext, SensitiveFieldTag,
    SensitiveModelTag,
};
use appdb::graph::{GraphCrud, GraphRepo};
use appdb::model::meta::{register_table, HasId, ModelMeta, ResolveRecordId, UniqueLookupMeta};
use appdb::model::relation::relation_name;
use appdb::query::{query_bound_return, RawSqlStmt};
use appdb::repository::Repo;
use appdb::tx::{run_tx, TxStmt};
use appdb::{Bridge, Crud, Id, Relation, Sensitive, Store, StoredModel};
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue, Table};
use tokio::runtime::Runtime;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItAliasedPost {
    id: Id,
    #[unique]
    slug: String,
    title: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
#[table_as(ItAliasedPost)]
struct ItAliasedPostBase {
    id: Id,
    #[unique]
    slug: String,
    title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItAliasedForeignParent {
    id: Id,
    #[foreign]
    featured: Option<ItAliasedPostBase>,
    #[foreign]
    nested: Option<Vec<Vec<ItAliasedPostBase>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredAliasedForeignParentRow {
    id: Id,
    featured: Option<RecordId>,
    nested: Option<Vec<Vec<RecordId>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItInlineChild {
    id: Id,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItInlineParent {
    id: Id,
    child: ItInlineChild,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredInlineParentRow {
    id: RecordId,
    child: ItInlineChild,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItNestedIdChild {
    id: Id,
    #[unique]
    name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItNestedLookupChild {
    #[unique]
    code: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItNestedParent {
    id: Id,
    #[foreign]
    child: ItNestedIdChild,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItNestedOptionalParent {
    id: Id,
    #[foreign]
    child: Option<ItNestedLookupChild>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredNestedParentRow {
    id: Id,
    child: RecordId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredNestedOptionalParentRow {
    id: Id,
    child: Option<RecordId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItNestedVecParent {
    id: Id,
    #[foreign]
    children: Vec<ItNestedLookupChild>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredNestedVecParentRow {
    id: Id,
    children: Vec<RecordId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItRecursiveForeignParent {
    id: Id,
    #[foreign]
    children: Option<Vec<Vec<ItNestedLookupChild>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredRecursiveForeignParentRow {
    id: Id,
    children: Option<Vec<Vec<RecordId>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItManualForeignAlphaChild {
    id: Id,
    #[unique]
    name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItManualForeignBetaChild {
    #[unique]
    code: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Bridge)]
enum ItManualForeignChild {
    Alpha(ItManualForeignAlphaChild),
    Beta(ItManualForeignBetaChild),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
struct ItManualForeignParent {
    id: Id,
    #[foreign]
    child: ItManualForeignChild,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredManualForeignParentRow {
    id: Id,
    child: RecordId,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store, Sensitive)]
struct ItSensitiveProfile {
    id: Id,
    alias: String,
    #[secure]
    secret: String,
    #[secure]
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store, Sensitive)]
struct ItSensitiveFallbackLookup {
    alias: String,
    #[secure]
    secret: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store, Sensitive)]
struct ItSensitiveLookupSource {
    #[unique]
    alias: String,
    #[secure]
    secret: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue, Store, Sensitive)]
struct ItSensitiveLookupTarget {
    #[unique]
    code: String,
    #[secure]
    secret: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
struct StoredSensitiveProfileRow {
    id: RecordId,
    alias: String,
    secret: Vec<u8>,
    note: Option<Vec<u8>>,
}

impl ModelMeta for ItRecordUser {
    fn table_name() -> &'static str {
        static TABLE_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        TABLE_NAME.get_or_init(|| register_table(stringify!(ItRecordUser), "it_record_user"))
    }
}

impl Crud for ItRecordUser {}

impl StoredModel for ItRecordUser {
    type Stored = Self;

    fn into_stored(self) -> anyhow::Result<Self::Stored> {
        Ok(self)
    }

    fn from_stored(stored: Self::Stored) -> anyhow::Result<Self> {
        Ok(stored)
    }
}

impl appdb::ForeignModel for ItRecordUser {
    async fn persist_foreign(value: Self) -> anyhow::Result<Self::Stored> {
        Ok(value)
    }

    async fn hydrate_foreign(stored: Self::Stored) -> anyhow::Result<Self> {
        Ok(stored)
    }
}

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
#[allow(dead_code)]
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
    reinit_db(test_db_path())
        .await
        .expect("database should initialize");
}

fn install_sensitive_test_contexts() {
    clear_crypto_context_registry();
    let ctx = CryptoContext::new([7; 32]).expect("test context should be valid");
    register_crypto_context_for::<AppdbSensitiveFieldTagItSensitiveProfileSecret>(ctx.clone());
    register_crypto_context_for::<AppdbSensitiveFieldTagItSensitiveProfileNote>(ctx);

    let ctx = CryptoContext::new([8; 32]).expect("lookup source context should be valid");
    register_crypto_context_for::<AppdbSensitiveFieldTagItSensitiveLookupSourceSecret>(ctx);

    let ctx = CryptoContext::new([9; 32]).expect("lookup target context should be valid");
    register_crypto_context_for::<AppdbSensitiveFieldTagItSensitiveLookupTargetSecret>(ctx);
}

async fn load_sensitive_profile_raw(id: &str) -> StoredSensitiveProfileRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItSensitiveProfile::table_name())
        .bind("id", id.to_owned());

    query_bound_return::<StoredSensitiveProfileRow>(stmt)
        .await
        .expect("raw row query should succeed")
        .expect("raw row should exist")
}

async fn load_aliased_foreign_parent_raw(id: &str) -> StoredAliasedForeignParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItAliasedForeignParent::table_name())
        .bind("id", id.to_owned());

    let value = query_bound_return::<serde_json::Value>(stmt)
        .await
        .expect("aliased foreign raw parent query should succeed")
        .expect("aliased foreign raw parent should exist");

    let mut map = value
        .as_object()
        .cloned()
        .expect("aliased foreign raw row should be an object");
    let featured = map
        .remove("featured")
        .map(serde_json::from_value::<Option<RecordId>>)
        .transpose()
        .expect("aliased foreign featured should decode as optional record id")
        .flatten();
    let nested = map
        .remove("nested")
        .map(serde_json::from_value::<Option<Vec<Vec<RecordId>>>>)
        .transpose()
        .expect("aliased foreign nested should decode as nested optional record ids")
        .flatten();
    StoredAliasedForeignParentRow {
        id: Id::from(id),
        featured,
        nested,
    }
}

async fn load_inline_parent_raw(id: &str) -> StoredInlineParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItInlineParent::table_name())
        .bind("id", id.to_owned());

    query_bound_return::<StoredInlineParentRow>(stmt)
        .await
        .expect("inline parent raw row query should succeed")
        .expect("inline parent raw row should exist")
}

async fn load_nested_parent_raw(id: &str) -> StoredNestedParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItNestedParent::table_name())
        .bind("id", id.to_owned());

    let value = query_bound_return::<serde_json::Value>(stmt)
        .await
        .expect("nested parent raw row query should succeed")
        .expect("nested parent raw row should exist");
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| {
            raw.split_once(':')
                .map(|(_, key)| key.trim_matches('`').to_owned())
        })
        .expect("nested parent raw row should contain normalized string id");
    let child = value
        .get("child")
        .cloned()
        .map(serde_json::from_value::<RecordId>)
        .expect("nested parent raw row should contain child")
        .expect("nested parent child should decode as record id");
    StoredNestedParentRow {
        id: Id::from(id),
        child,
    }
}

async fn load_nested_optional_parent_raw(id: &str) -> StoredNestedOptionalParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItNestedOptionalParent::table_name())
        .bind("id", id.to_owned());

    let value = query_bound_return::<serde_json::Value>(stmt)
        .await
        .expect("nested optional parent raw row query should succeed")
        .expect("nested optional parent raw row should exist");
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| {
            raw.split_once(':')
                .map(|(_, key)| key.trim_matches('`').to_owned())
        })
        .expect("nested optional parent raw row should contain normalized string id");
    let child = value
        .get("child")
        .cloned()
        .map(serde_json::from_value::<RecordId>)
        .transpose()
        .expect("nested optional parent child should decode as optional record id");
    StoredNestedOptionalParentRow {
        id: Id::from(id),
        child,
    }
}

async fn load_nested_vec_parent_raw(id: &str) -> StoredNestedVecParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItNestedVecParent::table_name())
        .bind("id", id.to_owned());

    let value = query_bound_return::<serde_json::Value>(stmt)
        .await
        .expect("nested vec parent raw row query should succeed")
        .expect("nested vec parent raw row should exist");
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| {
            raw.split_once(':')
                .map(|(_, key)| key.trim_matches('`').to_owned())
        })
        .expect("nested vec parent raw row should contain normalized string id");
    let children = value
        .get("children")
        .cloned()
        .map(serde_json::from_value::<Vec<RecordId>>)
        .expect("nested vec parent raw row should contain children")
        .expect("nested vec children should decode as record ids");
    StoredNestedVecParentRow {
        id: Id::from(id),
        children,
    }
}

async fn load_recursive_foreign_parent_raw(id: &str) -> StoredRecursiveForeignParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItRecursiveForeignParent::table_name())
        .bind("id", id.to_owned());

    let value = query_bound_return::<serde_json::Value>(stmt)
        .await
        .expect("recursive foreign parent raw row query should succeed")
        .expect("recursive foreign parent raw row should exist");
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| {
            raw.split_once(':')
                .map(|(_, key)| key.trim_matches('`').to_owned())
        })
        .expect("recursive foreign parent raw row should contain normalized string id");
    let children = value
        .get("children")
        .cloned()
        .map(serde_json::from_value::<Option<Vec<Vec<RecordId>>>>)
        .transpose()
        .expect("recursive foreign children should decode as nested optional record ids")
        .flatten();
    StoredRecursiveForeignParentRow {
        id: Id::from(id),
        children,
    }
}

async fn load_manual_foreign_parent_raw(id: &str) -> StoredManualForeignParentRow {
    let stmt = RawSqlStmt::new("SELECT * FROM type::record($table, $id);")
        .bind("table", ItManualForeignParent::table_name())
        .bind("id", id.to_owned());

    let value = query_bound_return::<serde_json::Value>(stmt)
        .await
        .expect("manual foreign parent raw row query should succeed")
        .expect("manual foreign parent raw row should exist");
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| {
            raw.split_once(':')
                .map(|(_, key)| key.trim_matches('`').to_owned())
        })
        .expect("manual foreign parent raw row should contain normalized string id");
    let child = value
        .get("child")
        .cloned()
        .map(serde_json::from_value::<RecordId>)
        .expect("manual foreign parent raw row should contain child")
        .expect("manual foreign child should decode as record id");
    StoredManualForeignParentRow {
        id: Id::from(id),
        child,
    }
}

fn assert_sensitive_row_encrypted(
    raw: &StoredSensitiveProfileRow,
    expected_alias: &str,
    expected_secret: &str,
    expected_note: Option<&str>,
) {
    assert_eq!(raw.alias, expected_alias);
    assert_ne!(raw.secret, expected_secret.as_bytes());
    assert!(raw.secret.len() > expected_secret.len());

    match (&raw.note, expected_note) {
        (Some(ciphertext), Some(plaintext)) => {
            assert_ne!(ciphertext, plaintext.as_bytes());
            assert!(ciphertext.len() > plaintext.len());
        }
        (None, None) => {}
        other => panic!("unexpected note state: {other:?}"),
    }
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

        let inserted = Repo::<ItNumberUser>::save(ItNumberUser {
            id: Id::from(42i64),
        })
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
fn nested_ref_opt_in_keeps_unannotated_fields_inline() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItInlineParent>::delete_all()
            .await
            .expect("delete_all should succeed");

        let input = ItInlineParent {
            id: Id::from("inline-parent"),
            child: ItInlineChild {
                id: Id::from("inline-child"),
                name: "alpha".to_owned(),
            },
        };

        let saved = ItInlineParent::save(input.clone())
            .await
            .expect("save should succeed");
        let loaded = ItInlineParent::get("inline-parent")
            .await
            .expect("get should succeed");
        let raw = load_inline_parent_raw("inline-parent").await;

        assert_eq!(saved, input);
        assert_eq!(loaded, input);
        assert_eq!(raw.child, input.child);
    });
}

#[test]
fn nested_ref_single_and_option_paths() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItNestedParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedIdChild>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedOptionalParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedIdChild>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedLookupChild>::delete_all()
            .await
            .expect("delete_all should succeed");

        let direct_existing_input = ItNestedParent {
            id: Id::from("nested-parent-id"),
            child: ItNestedIdChild {
                id: Id::from("nested-child-id"),
                name: "alpha".to_owned(),
            },
        };

        let direct_child_created =
            Repo::<ItNestedIdChild>::create(direct_existing_input.child.clone())
                .await
                .expect("direct child create should succeed");
        assert_eq!(
            direct_child_created
                .resolve_record_id()
                .await
                .expect("created child should resolve"),
            RecordId::new(ItNestedIdChild::table_name(), "nested-child-id")
        );

        let direct_existing_saved = ItNestedParent::save(direct_existing_input.clone())
            .await
            .expect("direct nested save should succeed");
        let direct_existing_loaded = ItNestedParent::get("nested-parent-id")
            .await
            .expect("direct nested get should succeed");
        let direct_existing_raw = load_nested_parent_raw("nested-parent-id").await;
        let direct_existing_child = Repo::<ItNestedIdChild>::get("nested-child-id")
            .await
            .expect("nested child row should exist");

        assert_eq!(direct_existing_saved, direct_existing_input);
        assert_eq!(direct_existing_loaded, direct_existing_input);
        assert_eq!(direct_existing_child, direct_existing_input.child);
        assert_eq!(
            direct_existing_raw.child,
            RecordId::new(ItNestedIdChild::table_name(), "nested-child-id")
        );

        let _seeded_lookup_child = Repo::<ItNestedLookupChild>::create(ItNestedLookupChild {
            code: "lookup-existing".to_owned(),
            note: Some("seeded".to_owned()),
        })
        .await
        .expect("seeded lookup child create should succeed");

        let reuse_parent = ItNestedOptionalParent {
            id: Id::from("nested-option-some-existing"),
            child: Some(ItNestedLookupChild {
                code: "lookup-existing".to_owned(),
                note: Some("seeded".to_owned()),
            }),
        };

        let reused_saved = ItNestedOptionalParent::save(reuse_parent.clone())
            .await
            .expect("lookup-backed nested save should succeed");
        let reused_loaded = ItNestedOptionalParent::get("nested-option-some-existing")
            .await
            .expect("lookup-backed nested get should succeed");
        let reused_raw = load_nested_optional_parent_raw("nested-option-some-existing").await;
        let lookup_ids = Repo::<ItNestedLookupChild>::list_record_ids()
            .await
            .expect("list_record_ids should succeed");

        assert_eq!(reused_saved, reuse_parent);
        assert_eq!(reused_loaded, reuse_parent);
        assert_eq!(lookup_ids.len(), 1);
        assert_eq!(reused_raw.child, Some(lookup_ids[0].clone()));

        let create_parent = ItNestedOptionalParent {
            id: Id::from("nested-option-some-create"),
            child: Some(ItNestedLookupChild {
                code: "lookup-create".to_owned(),
                note: Some("created".to_owned()),
            }),
        };

        let created_saved = ItNestedOptionalParent::save(create_parent.clone())
            .await
            .expect("missing lookup-backed nested save should create child");
        let created_loaded = ItNestedOptionalParent::get("nested-option-some-create")
            .await
            .expect("created lookup-backed nested get should succeed");
        let created_raw = load_nested_optional_parent_raw("nested-option-some-create").await;
        let created_child_id = Repo::<ItNestedLookupChild>::find_unique_id_for(
            create_parent
                .child
                .as_ref()
                .expect("child should be present"),
        )
        .await
        .expect("created lookup child should resolve");

        let explicit_create_parent = ItNestedOptionalParent {
            id: Id::from("nested-direct-create-through-option"),
            child: Some(ItNestedLookupChild {
                code: "lookup-create-explicit".to_owned(),
                note: Some("created-explicit".to_owned()),
            }),
        };

        let explicit_create_saved = ItNestedOptionalParent::save(explicit_create_parent.clone())
            .await
            .expect("missing optional nested child should be created");
        let explicit_create_loaded =
            ItNestedOptionalParent::get("nested-direct-create-through-option")
                .await
                .expect("created optional nested get should succeed");
        let explicit_create_raw =
            load_nested_optional_parent_raw("nested-direct-create-through-option").await;
        let explicit_create_child_id = Repo::<ItNestedLookupChild>::find_unique_id_for(
            explicit_create_parent
                .child
                .as_ref()
                .expect("child should be present"),
        )
        .await
        .expect("created optional nested child should resolve");
        let explicit_create_child =
            Repo::<ItNestedLookupChild>::get_record(explicit_create_child_id.clone())
                .await
                .expect("created optional nested child row should exist");

        assert_eq!(explicit_create_saved, explicit_create_parent);
        assert_eq!(explicit_create_loaded, explicit_create_parent);
        assert_eq!(
            explicit_create_child,
            explicit_create_parent
                .child
                .clone()
                .expect("child should be present")
        );
        assert_eq!(explicit_create_raw.child, Some(explicit_create_child_id));

        assert_eq!(created_saved, create_parent);
        assert_eq!(created_loaded, create_parent);
        let created_child = Repo::<ItNestedLookupChild>::get_record(created_child_id.clone())
            .await
            .expect("created lookup child should load");
        assert_eq!(
            created_child,
            create_parent
                .child
                .clone()
                .expect("child should be present")
        );
        assert_eq!(created_raw.child, Some(created_child_id.clone()));

        let none_parent = ItNestedOptionalParent {
            id: Id::from("nested-option-none"),
            child: None,
        };

        let none_saved = ItNestedOptionalParent::save(none_parent.clone())
            .await
            .expect("optional none nested save should succeed");
        let none_loaded = ItNestedOptionalParent::get("nested-option-none")
            .await
            .expect("optional none nested get should succeed");
        let none_raw = load_nested_optional_parent_raw("nested-option-none").await;
        let child_rows_after_none = Repo::<ItNestedLookupChild>::list()
            .await
            .expect("lookup child list should succeed");

        assert_eq!(none_saved, none_parent);
        assert_eq!(none_loaded, none_parent);
        assert_eq!(none_raw.child, None);
        assert_eq!(child_rows_after_none.len(), 3);
    });
}

#[test]
fn nested_ref_vec_and_collection_hydration() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItNestedVecParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedLookupChild>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedOptionalParent>::delete_all()
            .await
            .expect("delete_all should succeed");

        let existing_child = Repo::<ItNestedLookupChild>::create(ItNestedLookupChild {
            code: "vec-existing".to_owned(),
            note: Some("reused".to_owned()),
        })
        .await
        .expect("seeded vec child create should succeed");
        let existing_child_id = existing_child
            .resolve_record_id()
            .await
            .expect("existing vec child should resolve");

        let first_parent = ItNestedVecParent {
            id: Id::from("nested-vec-parent-1"),
            children: vec![
                existing_child.clone(),
                ItNestedLookupChild {
                    code: "vec-created-a".to_owned(),
                    note: Some("created-a".to_owned()),
                },
                ItNestedLookupChild {
                    code: "vec-created-b".to_owned(),
                    note: None,
                },
            ],
        };

        let first_saved = ItNestedVecParent::save(first_parent.clone())
            .await
            .expect("vec nested save should succeed");
        let first_loaded = ItNestedVecParent::get("nested-vec-parent-1")
            .await
            .expect("vec nested get should succeed");
        let first_raw = load_nested_vec_parent_raw("nested-vec-parent-1").await;

        let first_expected_ids = vec![
            existing_child_id.clone(),
            Repo::<ItNestedLookupChild>::find_unique_id_for(&first_parent.children[1])
                .await
                .expect("created vec child A should resolve"),
            Repo::<ItNestedLookupChild>::find_unique_id_for(&first_parent.children[2])
                .await
                .expect("created vec child B should resolve"),
        ];

        assert_eq!(first_saved, first_parent);
        assert_eq!(first_loaded, first_parent);
        assert_eq!(first_raw.children, first_expected_ids);

        let direct_parent = ItNestedParent {
            id: Id::from("nested-vec-direct-parent"),
            child: ItNestedIdChild {
                id: Id::from("nested-vec-direct-child"),
                name: "direct-alpha".to_owned(),
            },
        };
        let optional_parent = ItNestedOptionalParent {
            id: Id::from("nested-vec-optional-parent"),
            child: Some(ItNestedLookupChild {
                code: "vec-optional".to_owned(),
                note: Some("optional".to_owned()),
            }),
        };
        let second_parent = ItNestedVecParent {
            id: Id::from("nested-vec-parent-2"),
            children: vec![
                ItNestedLookupChild {
                    code: "vec-second-a".to_owned(),
                    note: Some("second-a".to_owned()),
                },
                existing_child.clone(),
            ],
        };

        let _direct_child_created = Repo::<ItNestedIdChild>::create(direct_parent.child.clone())
            .await
            .expect("direct child create should succeed");

        let direct_saved = ItNestedParent::save(direct_parent.clone())
            .await
            .expect("direct nested save should succeed");
        let optional_saved = ItNestedOptionalParent::save(optional_parent.clone())
            .await
            .expect("optional nested save should succeed");
        let second_saved = ItNestedVecParent::save(second_parent.clone())
            .await
            .expect("second vec nested save should succeed");
        let second_raw = load_nested_vec_parent_raw("nested-vec-parent-2").await;
        let second_expected_ids = vec![
            Repo::<ItNestedLookupChild>::find_unique_id_for(&second_parent.children[0])
                .await
                .expect("second vec child A should resolve"),
            existing_child_id.clone(),
        ];

        assert_eq!(direct_saved, direct_parent);
        assert_eq!(optional_saved, optional_parent);
        assert_eq!(second_saved, second_parent);
        assert_eq!(second_raw.children, second_expected_ids);

        let all_children = Repo::<ItNestedLookupChild>::list()
            .await
            .expect("lookup child list should succeed");
        assert_eq!(all_children.len(), 5);

        let vec_list = ItNestedVecParent::list()
            .await
            .expect("vec nested list should succeed");
        assert_eq!(vec_list.len(), 2);
        assert!(vec_list.contains(&first_parent));
        assert!(vec_list.contains(&second_parent));

        let direct_list = ItNestedParent::list()
            .await
            .expect("direct nested list should succeed");
        assert_eq!(direct_list, vec![direct_parent.clone()]);

        let optional_list = ItNestedOptionalParent::list()
            .await
            .expect("optional nested list should succeed");
        assert_eq!(optional_list, vec![optional_parent.clone()]);

        let vec_limited = ItNestedVecParent::list_limit(1)
            .await
            .expect("vec nested list_limit should succeed");
        assert_eq!(vec_limited.len(), 1);
        assert!(vec_limited[0] == first_parent || vec_limited[0] == second_parent);

        let direct_limited = ItNestedParent::list_limit(1)
            .await
            .expect("direct nested list_limit should succeed");
        assert_eq!(direct_limited, vec![direct_parent]);

        let optional_limited = ItNestedOptionalParent::list_limit(1)
            .await
            .expect("optional nested list_limit should succeed");
        assert_eq!(optional_limited, vec![optional_parent]);
    });
}

#[test]
fn nested_ref_recursive_option_vec_shapes_roundtrip() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItRecursiveForeignParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedLookupChild>::delete_all()
            .await
            .expect("delete_all should succeed");

        let parent = ItRecursiveForeignParent {
            id: Id::from("recursive-foreign-parent"),
            children: Some(vec![
                vec![
                    ItNestedLookupChild {
                        code: "recursive-a".to_owned(),
                        note: Some("note-a".to_owned()),
                    },
                    ItNestedLookupChild {
                        code: "recursive-b".to_owned(),
                        note: None,
                    },
                ],
                vec![ItNestedLookupChild {
                    code: "recursive-c".to_owned(),
                    note: Some("note-c".to_owned()),
                }],
            ]),
        };

        let saved = ItRecursiveForeignParent::save(parent.clone())
            .await
            .expect("recursive foreign save should succeed");
        let loaded = ItRecursiveForeignParent::get("recursive-foreign-parent")
            .await
            .expect("recursive foreign get should succeed");
        let listed = ItRecursiveForeignParent::list()
            .await
            .expect("recursive foreign list should succeed");
        let raw = load_recursive_foreign_parent_raw("recursive-foreign-parent").await;

        let expected_ids = parent
            .children
            .as_ref()
            .expect("children should be present")
            .iter()
            .map(|row| async move {
                let mut ids = Vec::with_capacity(row.len());
                for child in row {
                    ids.push(
                        Repo::<ItNestedLookupChild>::find_unique_id_for(child)
                            .await
                            .expect("recursive child should resolve"),
                    );
                }
                ids
            });

        let mut resolved_nested_ids = Vec::new();
        for future in expected_ids {
            resolved_nested_ids.push(future.await);
        }

        assert_eq!(saved, parent);
        assert_eq!(loaded, parent);
        assert_eq!(listed, vec![parent.clone()]);
        assert_eq!(raw.children, Some(resolved_nested_ids));
    });
}

#[test]
fn manual_foreign_enum_dispatcher_roundtrip_passes() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItManualForeignParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItManualForeignAlphaChild>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItManualForeignBetaChild>::delete_all()
            .await
            .expect("delete_all should succeed");

        let alpha_parent = ItManualForeignParent {
            id: Id::from("manual-foreign-alpha-parent"),
            child: ItManualForeignChild::Alpha(ItManualForeignAlphaChild {
                id: Id::from("manual-foreign-alpha-child"),
                name: "alpha".to_owned(),
            }),
        };

        let alpha_child = match &alpha_parent.child {
            ItManualForeignChild::Alpha(child) => child.clone(),
            ItManualForeignChild::Beta(_) => {
                unreachable!("alpha parent should contain alpha child")
            }
        };

        Repo::<ItManualForeignAlphaChild>::create(alpha_child)
            .await
            .expect("alpha child seed should succeed");

        let saved_alpha = ItManualForeignParent::save(alpha_parent.clone())
            .await
            .expect("alpha foreign save should succeed");
        let loaded_alpha = ItManualForeignParent::get("manual-foreign-alpha-parent")
            .await
            .expect("alpha foreign get should succeed");
        let raw_alpha = load_manual_foreign_parent_raw("manual-foreign-alpha-parent").await;

        assert_eq!(saved_alpha, alpha_parent);
        assert_eq!(loaded_alpha, alpha_parent);
        assert_eq!(
            raw_alpha.child,
            RecordId::new(
                ItManualForeignAlphaChild::table_name(),
                "manual-foreign-alpha-child"
            )
        );

        let beta_parent = ItManualForeignParent {
            id: Id::from("manual-foreign-beta-parent"),
            child: ItManualForeignChild::Beta(ItManualForeignBetaChild {
                code: "manual-foreign-beta-child".to_owned(),
                note: Some("beta-note".to_owned()),
            }),
        };

        let saved_beta = ItManualForeignParent::save(beta_parent.clone())
            .await
            .expect("beta foreign save should succeed");
        let loaded_beta = ItManualForeignParent::get("manual-foreign-beta-parent")
            .await
            .expect("beta foreign get should succeed");
        let raw_beta = load_manual_foreign_parent_raw("manual-foreign-beta-parent").await;
        let beta_child_id =
            Repo::<ItManualForeignBetaChild>::find_unique_id_for(match &beta_parent.child {
                ItManualForeignChild::Beta(child) => child,
                ItManualForeignChild::Alpha(_) => {
                    unreachable!("beta parent should contain beta child")
                }
            })
            .await
            .expect("beta child id should resolve");

        assert_eq!(saved_beta, beta_parent);
        assert_eq!(loaded_beta, beta_parent);
        assert_eq!(raw_beta.child, beta_child_id);
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

    assert!(ddls.iter().any(|ddl| {
        ddl.contains("DEFINE INDEX IF NOT EXISTS it_aliased_post_slug_unique")
            && ddl.contains("ON it_aliased_post")
            && ddl.contains("FIELDS slug UNIQUE")
    }));
}

#[test]
fn table_as_reuses_target_table_name_and_lookup_metadata() {
    assert_eq!(ItAliasedPost::table_name(), ItAliasedPostBase::table_name());
    assert_eq!(ItAliasedPostBase::lookup_fields(), &["slug"]);
}

#[test]
fn table_as_roundtrip_and_foreign_paths_share_target_table() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;

        Repo::<ItAliasedForeignParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItAliasedPost>::delete_all()
            .await
            .expect("delete_all should succeed");

        let post = ItAliasedPost {
            id: Id::from("aliased-post"),
            slug: "post-slug".to_owned(),
            title: "Full title".to_owned(),
            body: "Full body".to_owned(),
        };

        let saved_post = ItAliasedPost::save(post.clone())
            .await
            .expect("target save should succeed");
        let post_base = ItAliasedPostBase::get("aliased-post")
            .await
            .expect("alias get should succeed on target table");
        assert_eq!(saved_post.id, post_base.id);
        assert_eq!(saved_post.slug, post_base.slug);
        assert_eq!(saved_post.title, post_base.title);

        let alias_lookup = Repo::<ItAliasedPostBase>::find_unique_id_for(&ItAliasedPostBase {
            id: Id::from("ignored"),
            slug: "post-slug".to_owned(),
            title: "Different title ignored by lookup".to_owned(),
        })
        .await
        .expect("alias lookup should resolve on shared table");
        assert_eq!(
            alias_lookup,
            RecordId::new(ItAliasedPost::table_name(), "aliased-post")
        );

        let parent = ItAliasedForeignParent {
            id: Id::from("aliased-parent"),
            featured: Some(ItAliasedPostBase {
                id: Id::from("aliased-post"),
                slug: "post-slug".to_owned(),
                title: "Full title".to_owned(),
            }),
            nested: Some(vec![vec![ItAliasedPostBase {
                id: Id::from("aliased-post"),
                slug: "post-slug".to_owned(),
                title: "Full title".to_owned(),
            }]]),
        };

        let saved_parent = ItAliasedForeignParent::save(parent.clone())
            .await
            .expect("aliased foreign parent save should succeed");
        let loaded_parent = ItAliasedForeignParent::get("aliased-parent")
            .await
            .expect("aliased foreign parent get should succeed");
        let raw_parent = load_aliased_foreign_parent_raw("aliased-parent").await;

        assert_eq!(saved_parent, parent);
        assert_eq!(loaded_parent, parent);

        let expected_record = RecordId::new(ItAliasedPost::table_name(), "aliased-post");
        assert_eq!(raw_parent.featured, Some(expected_record.clone()));
        assert_eq!(raw_parent.nested, Some(vec![vec![expected_record]]));
    });
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
fn sensitive_store_lookup_metadata_excludes_secure_fields_from_fallback() {
    assert_eq!(
        ItSensitiveFallbackLookup::lookup_fields(),
        &["alias", "note"]
    );
}

#[test]
fn mixed_sensitive_models_resolve_unique_ids_through_non_secure_fields() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");

        let source = Repo::<ItSensitiveLookupSource>::create(ItSensitiveLookupSource {
            alias: "sensitive-source".to_owned(),
            secret: "source-secret".to_owned(),
            note: Some("source-note".to_owned()),
        })
        .await
        .expect("source create should succeed");

        assert_eq!(ItSensitiveLookupSource::lookup_fields(), &["alias"]);

        let resolved = Repo::<ItSensitiveLookupSource>::find_unique_id_for(&source)
            .await
            .expect("mixed model lookup should resolve via non-secure unique field");

        assert_eq!(
            resolved,
            source
                .resolve_record_id()
                .await
                .expect("resolve_record_id should use the non-secure lookup path")
        );
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
fn relation_type_api_accepts_mixed_sensitive_store_models_without_exposing_encrypted_types() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItSensitiveLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");

        let source = Repo::<ItSensitiveLookupSource>::create(ItSensitiveLookupSource {
            alias: "sensitive-type-source".to_owned(),
            secret: "source-secret".to_owned(),
            note: Some("source-note".to_owned()),
        })
        .await
        .expect("source create should succeed");
        let target = Repo::<ItSensitiveLookupTarget>::create(ItSensitiveLookupTarget {
            code: "sensitive-type-target".to_owned(),
            secret: "target-secret".to_owned(),
            note: Some("target-note".to_owned()),
        })
        .await
        .expect("target create should succeed");

        assert_eq!(ItSensitiveLookupSource::lookup_fields(), &["alias"]);
        assert_eq!(ItSensitiveLookupTarget::lookup_fields(), &["code"]);

        ItFollowsRel::relate(&source, &target)
            .await
            .expect("type relate should resolve mixed sensitive models through legal non-secure lookup fields");

        let target_id = target
            .resolve_record_id()
            .await
            .expect("target resolve_record_id should use the legal non-secure lookup field");

        let outs = ItFollowsRel::out_ids(&source, ItSensitiveLookupTarget::table_name())
            .await
            .expect("type out_ids should succeed for mixed sensitive models");
        assert!(outs.iter().any(|id| id == &target_id));

        ItFollowsRel::unrelate(&source, &target)
            .await
            .expect("type unrelate should resolve mixed sensitive models through legal non-secure lookup fields");

        let outs_after = ItFollowsRel::out_ids(&source, ItSensitiveLookupTarget::table_name())
            .await
            .expect("type out_ids should still succeed after unrelate");
        assert!(!outs_after.iter().any(|id| id == &target_id));
    });
}

#[test]
fn relation_derive_registers_default_and_overridden_names() {
    assert_eq!(relation_name::<ItFollowsRel>(), "it_follows_rel");
    assert_eq!(
        relation_name::<AutoNamedTestRelation>(),
        "auto_named_test_relation"
    );
    assert_eq!(
        relation_name::<NamedFieldTestRelation>(),
        "named_field_test_relation"
    );
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

#[test]
fn store_sensitive_save_get_roundtrip_encrypts_at_rest() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let input = ItSensitiveProfile {
            id: Id::from("s-save"),
            alias: "alpha".to_owned(),
            secret: "swordfish".to_owned(),
            note: Some("memo".to_owned()),
        };

        let saved = ItSensitiveProfile::save(input.clone())
            .await
            .expect("save should succeed");
        let loaded = ItSensitiveProfile::get("s-save")
            .await
            .expect("get should succeed");

        assert_eq!(saved, input);
        assert_eq!(loaded, input);

        let raw = load_sensitive_profile_raw("s-save").await;
        assert_sensitive_row_encrypted(&raw, "alpha", "swordfish", Some("memo"));
    });
}

#[test]
fn nested_ref_cross_area_regressions() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItNestedParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedIdChild>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedOptionalParent>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItNestedLookupChild>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItProfile>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItFallbackLookup>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItSensitiveLookupSource>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItSensitiveLookupTarget>::delete_all()
            .await
            .expect("delete_all should succeed");

        let direct_input = ItNestedParent {
            id: Id::from("cross-area-parent"),
            child: ItNestedIdChild {
                id: Id::from("cross-area-child"),
                name: "cross-alpha".to_owned(),
            },
        };

        let seeded_child = Repo::<ItNestedIdChild>::create(direct_input.child.clone())
            .await
            .expect("direct child seed should succeed");
        assert_eq!(
            seeded_child
                .resolve_record_id()
                .await
                .expect("seeded direct child should resolve"),
            RecordId::new(ItNestedIdChild::table_name(), "cross-area-child")
        );

        let saved_parent = ItNestedParent::save(direct_input.clone())
            .await
            .expect("nested parent save should succeed");
        let loaded_parent = ItNestedParent::get("cross-area-parent")
            .await
            .expect("nested parent get should succeed");
        let raw_parent = load_nested_parent_raw("cross-area-parent").await;

        assert_eq!(saved_parent, direct_input);
        assert_eq!(loaded_parent, direct_input);
        assert_eq!(
            raw_parent.child,
            RecordId::new(ItNestedIdChild::table_name(), "cross-area-child")
        );
        let parent_row_json =
            serde_json::to_value(&raw_parent).expect("nested parent raw row should serialize");
        let parent_fields = parent_row_json
            .as_object()
            .expect("nested parent raw row should be an object");
        assert!(!parent_fields.contains_key("name"));

        let sensitive = ItSensitiveProfile::save(ItSensitiveProfile {
            id: Id::from("cross-sensitive"),
            alias: "cross-sensitive-alias".to_owned(),
            secret: "cross-sensitive-secret".to_owned(),
            note: Some("cross-sensitive-note".to_owned()),
        })
        .await
        .expect("sensitive save should succeed");
        assert_eq!(sensitive.secret, "cross-sensitive-secret");
        let raw_sensitive = load_sensitive_profile_raw("cross-sensitive").await;
        assert_sensitive_row_encrypted(
            &raw_sensitive,
            "cross-sensitive-alias",
            "cross-sensitive-secret",
            Some("cross-sensitive-note"),
        );

        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup-cross".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("first duplicate child create should succeed");
        Repo::<ItFallbackLookup>::create(ItFallbackLookup {
            name: "dup-cross".to_owned(),
            note: Some("same".to_owned()),
        })
        .await
        .expect("second duplicate child create should succeed");

        let child_ids_before = Repo::<ItFallbackLookup>::list_record_ids()
            .await
            .expect("duplicate child ids before relation checks should load");
        let ambiguous_err = ItFallbackLookup {
            name: "dup-cross".to_owned(),
            note: Some("same".to_owned()),
        }
        .resolve_record_id()
        .await
        .expect_err("ambiguous child lookup should fail");
        assert!(
            ambiguous_err.to_string().contains("multiple records"),
            "{ambiguous_err}"
        );
        let child_ids_after = Repo::<ItFallbackLookup>::list_record_ids()
            .await
            .expect("duplicate child ids after relation checks should load");
        assert_eq!(child_ids_after, child_ids_before);

        let plain_created_id = Repo::<ItProfile>::create_return_id(ItProfile {
            id: Id::from("cross-plain-create-return-id"),
            name: "cross-plain".to_owned(),
            note: Some("works".to_owned()),
        })
        .await
        .expect("plain create_return_id should succeed");
        assert_eq!(
            plain_created_id,
            RecordId::new(ItProfile::table_name(), "cross-plain-create-return-id")
        );
        let guarded_err = Repo::<ItSensitiveProfile>::create_return_id(ItSensitiveProfile {
            id: Id::from("cross-sensitive-create-return-id"),
            alias: "guarded".to_owned(),
            secret: "blocked".to_owned(),
            note: Some("nope".to_owned()),
        })
        .await
        .expect_err("sensitive create_return_id should remain guarded");
        assert!(guarded_err
            .to_string()
            .contains("does not support create_return_id"));

        let source = Repo::<ItLookupSource>::create(ItLookupSource {
            name: "cross-source".to_owned(),
        })
        .await
        .expect("relation source create should succeed");
        let target = Repo::<ItLookupTarget>::create(ItLookupTarget {
            code: "cross-target".to_owned(),
        })
        .await
        .expect("relation target create should succeed");
        ItFollowsRel::relate(&source, &target)
            .await
            .expect("relation helper should succeed");
        let target_id = target
            .resolve_record_id()
            .await
            .expect("relation target should resolve");
        let outs = ItFollowsRel::out_ids(&source, ItLookupTarget::table_name())
            .await
            .expect("relation out_ids should succeed");
        assert!(outs.iter().any(|id| id == &target_id));

        let relation_ambiguous_err = ItFollowsRel::out_ids(
            &ItFallbackLookup {
                name: "dup-cross".to_owned(),
                note: Some("same".to_owned()),
            },
            ItLookupTarget::table_name(),
        )
        .await
        .expect_err("relation helper should preserve ambiguous error path");
        assert!(
            relation_ambiguous_err
                .to_string()
                .contains("multiple records"),
            "{relation_ambiguous_err}"
        );

        let missing_target_err = source
            .relate::<ItFollowsRel, _>(&ItLookupTarget {
                code: "cross-missing-target".to_owned(),
            })
            .await
            .expect_err("relation helper should preserve missing target error path");
        assert!(
            missing_target_err.to_string().contains("Record not found"),
            "{missing_target_err}"
        );

        let sensitive_source = Repo::<ItSensitiveLookupSource>::create(ItSensitiveLookupSource {
            alias: "cross-sensitive-source".to_owned(),
            secret: "cross-sensitive-source-secret".to_owned(),
            note: Some("cross-sensitive-source-note".to_owned()),
        })
        .await
        .expect("sensitive relation source create should succeed");
        let sensitive_target = Repo::<ItSensitiveLookupTarget>::create(ItSensitiveLookupTarget {
            code: "cross-sensitive-target".to_owned(),
            secret: "cross-sensitive-target-secret".to_owned(),
            note: Some("cross-sensitive-target-note".to_owned()),
        })
        .await
        .expect("sensitive relation target create should succeed");
        ItFollowsRel::relate(&sensitive_source, &sensitive_target)
            .await
            .expect("mixed sensitive relation helper should succeed");
        let sensitive_target_id = sensitive_target
            .resolve_record_id()
            .await
            .expect("sensitive target should resolve via legal lookup");
        let sensitive_outs =
            ItFollowsRel::out_ids(&sensitive_source, ItSensitiveLookupTarget::table_name())
                .await
                .expect("mixed sensitive relation out_ids should succeed");
        assert!(sensitive_outs.iter().any(|id| id == &sensitive_target_id));
    });
}

#[test]
fn store_sensitive_save_get_uses_generated_resolver_tags_without_call_site_context() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        clear_crypto_context_registry();

        let secret_ctx = CryptoContext::new([10; 32]).expect("secret context should be valid");
        let note_ctx = CryptoContext::new([11; 32]).expect("note context should be valid");

        assert_eq!(
            <AppdbSensitiveFieldTagItSensitiveProfileSecret as SensitiveFieldTag>::model_tag(),
            <ItSensitiveProfile as SensitiveModelTag>::model_tag()
        );
        assert_eq!(
            <AppdbSensitiveFieldTagItSensitiveProfileSecret as SensitiveFieldTag>::field_tag(),
            "secret"
        );
        assert_eq!(
            <AppdbSensitiveFieldTagItSensitiveProfileNote as SensitiveFieldTag>::model_tag(),
            <ItSensitiveProfile as SensitiveModelTag>::model_tag()
        );
        assert_eq!(
            <AppdbSensitiveFieldTagItSensitiveProfileNote as SensitiveFieldTag>::field_tag(),
            "note"
        );

        register_crypto_context_for::<AppdbSensitiveFieldTagItSensitiveProfileSecret>(secret_ctx);
        register_crypto_context_for::<AppdbSensitiveFieldTagItSensitiveProfileNote>(note_ctx);

        let input = ItSensitiveProfile {
            id: Id::from("s-runtime-tags"),
            alias: "runtime-tags".to_owned(),
            secret: "resolver-secret".to_owned(),
            note: Some("resolver-note".to_owned()),
        };

        let saved = ItSensitiveProfile::save(input.clone())
            .await
            .expect("save should resolve contexts from generated field tags");
        let loaded = ItSensitiveProfile::get("s-runtime-tags")
            .await
            .expect("get should decrypt through generated field tags");

        assert_eq!(saved, input);
        assert_eq!(loaded, input);

        let raw = load_sensitive_profile_raw("s-runtime-tags").await;
        assert_sensitive_row_encrypted(
            &raw,
            "runtime-tags",
            "resolver-secret",
            Some("resolver-note"),
        );
    });
}

#[test]
fn store_sensitive_list_and_list_limit_return_plaintext_models() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let first = ItSensitiveProfile {
            id: Id::from("s-list-1"),
            alias: "alpha".to_owned(),
            secret: "one".to_owned(),
            note: Some("first".to_owned()),
        };
        let second = ItSensitiveProfile {
            id: Id::from("s-list-2"),
            alias: "beta".to_owned(),
            secret: "two".to_owned(),
            note: None,
        };

        ItSensitiveProfile::save_many(vec![first.clone(), second.clone()])
            .await
            .expect("save_many should succeed");

        let listed = ItSensitiveProfile::list()
            .await
            .expect("list should succeed");
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&first));
        assert!(listed.contains(&second));

        let limited = ItSensitiveProfile::list_limit(1)
            .await
            .expect("list_limit should succeed");
        assert_eq!(limited.len(), 1);
        assert!(limited[0] == first || limited[0] == second);

        let raw_first = load_sensitive_profile_raw("s-list-1").await;
        let raw_second = load_sensitive_profile_raw("s-list-2").await;
        assert_sensitive_row_encrypted(&raw_first, "alpha", "one", Some("first"));
        assert_sensitive_row_encrypted(&raw_second, "beta", "two", None);
    });
}

#[test]
fn store_sensitive_create_and_create_at_keep_plaintext_surface() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let created = Repo::<ItSensitiveProfile>::create(ItSensitiveProfile {
            id: Id::from("s-create"),
            alias: "gamma".to_owned(),
            secret: "three".to_owned(),
            note: Some("create".to_owned()),
        })
        .await
        .expect("create should succeed");
        assert_eq!(created.secret, "three");

        let create_at_id = RecordId::new(ItSensitiveProfile::table_name(), "s-create-at");
        let created_at = Repo::<ItSensitiveProfile>::create_at(
            create_at_id,
            ItSensitiveProfile {
                id: Id::from("s-create-at"),
                alias: "delta".to_owned(),
                secret: "four".to_owned(),
                note: None,
            },
        )
        .await
        .expect("create_at should succeed");
        assert_eq!(created_at.alias, "delta");
        assert_eq!(created_at.secret, "four");

        let raw_created = load_sensitive_profile_raw("s-create").await;
        let raw_created_at = load_sensitive_profile_raw("s-create-at").await;
        assert_sensitive_row_encrypted(&raw_created, "gamma", "three", Some("create"));
        assert_sensitive_row_encrypted(&raw_created_at, "delta", "four", None);
    });
}

#[test]
fn store_create_return_id_keeps_plain_store_behavior_and_guards_sensitive_models() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItProfile>::delete_all()
            .await
            .expect("delete_all should succeed");
        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let created_id = Repo::<ItProfile>::create_return_id(ItProfile {
            id: Id::from("plain-create-return-id"),
            name: "plain".to_owned(),
            note: Some("works".to_owned()),
        })
        .await
        .expect("plain Store create_return_id should succeed");
        assert_eq!(
            created_id,
            RecordId::new(ItProfile::table_name(), "plain-create-return-id")
        );

        let loaded = Repo::<ItProfile>::get("plain-create-return-id")
            .await
            .expect("plain Store row should exist after create_return_id");
        assert_eq!(loaded.name, "plain");
        assert_eq!(loaded.note.as_deref(), Some("works"));

        let err = Repo::<ItSensitiveProfile>::create_return_id(ItSensitiveProfile {
            id: Id::from("sensitive-create-return-id"),
            alias: "guarded".to_owned(),
            secret: "blocked".to_owned(),
            note: Some("nope".to_owned()),
        })
        .await
        .expect_err("sensitive Store create_return_id should be guarded");

        let message = err.to_string();
        assert!(message.contains("does not support create_return_id"));
        assert!(message.contains("use create or create_at instead"));

        let listed = Repo::<ItSensitiveProfile>::list().await;
        match listed {
            Ok(rows) => assert!(rows.is_empty()),
            Err(err) => assert!(
                err.to_string().contains("does not exist"),
                "unexpected error after guarded sensitive create_return_id: {err}"
            ),
        }
    });
}

#[test]
fn store_sensitive_upsert_and_update_paths_replace_ciphertext() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let upserted = Repo::<ItSensitiveProfile>::upsert(ItSensitiveProfile {
            id: Id::from("s-upsert"),
            alias: "before".to_owned(),
            secret: "alpha-secret".to_owned(),
            note: Some("alpha-note".to_owned()),
        })
        .await
        .expect("upsert should succeed");
        assert_eq!(upserted.secret, "alpha-secret");
        let raw_before = load_sensitive_profile_raw("s-upsert").await;

        let upserted_at = Repo::<ItSensitiveProfile>::upsert_at(
            RecordId::new(ItSensitiveProfile::table_name(), "s-upsert-at"),
            ItSensitiveProfile {
                id: Id::from("s-upsert-at"),
                alias: "beta".to_owned(),
                secret: "beta-secret".to_owned(),
                note: None,
            },
        )
        .await
        .expect("upsert_at should succeed");
        assert_eq!(upserted_at.secret, "beta-secret");

        let updated = Repo::<ItSensitiveProfile>::update_at(
            RecordId::new(ItSensitiveProfile::table_name(), "s-upsert"),
            ItSensitiveProfile {
                id: Id::from("s-upsert"),
                alias: "after".to_owned(),
                secret: "omega-secret".to_owned(),
                note: Some("omega-note".to_owned()),
            },
        )
        .await
        .expect("update_at should succeed");
        assert_eq!(updated.alias, "after");
        assert_eq!(updated.secret, "omega-secret");

        let reloaded = Repo::<ItSensitiveProfile>::get("s-upsert")
            .await
            .expect("get should succeed");
        assert_eq!(reloaded, updated);

        let raw_after = load_sensitive_profile_raw("s-upsert").await;
        assert_sensitive_row_encrypted(&raw_after, "after", "omega-secret", Some("omega-note"));
        assert_ne!(raw_before.secret, raw_after.secret);

        let raw_upsert_at = load_sensitive_profile_raw("s-upsert-at").await;
        assert_sensitive_row_encrypted(&raw_upsert_at, "beta", "beta-secret", None);
    });
}

#[test]
fn store_sensitive_save_many_and_insert_return_plaintext_models() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        let saved_many = Repo::<ItSensitiveProfile>::save_many(vec![
            ItSensitiveProfile {
                id: Id::from("s-many-1"),
                alias: "alpha".to_owned(),
                secret: "one".to_owned(),
                note: Some("batch".to_owned()),
            },
            ItSensitiveProfile {
                id: Id::from("s-many-2"),
                alias: "beta".to_owned(),
                secret: "two".to_owned(),
                note: None,
            },
        ])
        .await
        .expect("save_many should succeed");
        assert_eq!(saved_many.len(), 2);
        assert_eq!(saved_many[0].secret, "one");
        assert_eq!(saved_many[1].secret, "two");

        let inserted = Repo::<ItSensitiveProfile>::insert(vec![
            ItSensitiveProfile {
                id: Id::from("s-insert-1"),
                alias: "gamma".to_owned(),
                secret: "three".to_owned(),
                note: Some("inserted".to_owned()),
            },
            ItSensitiveProfile {
                id: Id::from("s-insert-2"),
                alias: "delta".to_owned(),
                secret: "four".to_owned(),
                note: None,
            },
        ])
        .await
        .expect("insert should succeed");
        assert_eq!(inserted.len(), 2);
        assert_eq!(inserted[0].secret, "three");
        assert_eq!(inserted[1].secret, "four");

        let listed = Repo::<ItSensitiveProfile>::list()
            .await
            .expect("list should succeed");
        assert_eq!(listed.len(), 4);

        assert_sensitive_row_encrypted(
            &load_sensitive_profile_raw("s-many-1").await,
            "alpha",
            "one",
            Some("batch"),
        );
        assert_sensitive_row_encrypted(
            &load_sensitive_profile_raw("s-insert-1").await,
            "gamma",
            "three",
            Some("inserted"),
        );
    });
}

#[test]
fn store_sensitive_insert_ignore_and_insert_or_replace_preserve_plaintext_semantics() {
    let _guard = acquire_test_lock();
    run_async(async {
        ensure_db().await;
        install_sensitive_test_contexts();

        Repo::<ItSensitiveProfile>::delete_all()
            .await
            .expect("delete_all should succeed");

        Repo::<ItSensitiveProfile>::insert(vec![ItSensitiveProfile {
            id: Id::from("s-conflict"),
            alias: "original".to_owned(),
            secret: "original-secret".to_owned(),
            note: Some("original-note".to_owned()),
        }])
        .await
        .expect("seed insert should succeed");

        let ignored = Repo::<ItSensitiveProfile>::insert_ignore(vec![
            ItSensitiveProfile {
                id: Id::from("s-conflict"),
                alias: "ignored".to_owned(),
                secret: "ignored-secret".to_owned(),
                note: Some("ignored-note".to_owned()),
            },
            ItSensitiveProfile {
                id: Id::from("s-new"),
                alias: "new".to_owned(),
                secret: "new-secret".to_owned(),
                note: None,
            },
        ])
        .await
        .expect("insert_ignore should succeed");
        assert_eq!(ignored.len(), 1);
        assert_eq!(ignored[0].id, Id::from("s-new"));

        let after_ignore = Repo::<ItSensitiveProfile>::get("s-conflict")
            .await
            .expect("conflict row should still exist");
        assert_eq!(after_ignore.alias, "original");
        assert_eq!(after_ignore.secret, "original-secret");

        let replaced = Repo::<ItSensitiveProfile>::insert_or_replace(vec![
            ItSensitiveProfile {
                id: Id::from("s-conflict"),
                alias: "replacement".to_owned(),
                secret: "replacement-secret".to_owned(),
                note: Some("replacement-note".to_owned()),
            },
            ItSensitiveProfile {
                id: Id::from("s-fresh"),
                alias: "fresh".to_owned(),
                secret: "fresh-secret".to_owned(),
                note: None,
            },
        ])
        .await
        .expect("insert_or_replace should succeed");
        assert_eq!(replaced.len(), 2);
        assert!(replaced
            .iter()
            .any(|row| row.id == Id::from("s-conflict") && row.secret == "replacement-secret"));

        let after_replace = Repo::<ItSensitiveProfile>::get("s-conflict")
            .await
            .expect("replaced row should load");
        assert_eq!(after_replace.alias, "replacement");
        assert_eq!(after_replace.secret, "replacement-secret");

        assert_sensitive_row_encrypted(
            &load_sensitive_profile_raw("s-conflict").await,
            "replacement",
            "replacement-secret",
            Some("replacement-note"),
        );
        assert_sensitive_row_encrypted(
            &load_sensitive_profile_raw("s-new").await,
            "new",
            "new-secret",
            None,
        );
    });
}
