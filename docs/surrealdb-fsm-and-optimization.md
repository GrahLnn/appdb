# SurrealDB Best Practices, Function FSMs, and Optimization Review

Date: 2026-03-14
Scope: `core` and `macros` workspace after the SurrealDB `3.0.4` upgrade.

## Why this document exists

This library is already moving in the right direction as a capability-layered infrastructure crate with a facade export surface. That is the right shape for this codebase. It does not need a DDD rewrite first. It needs stronger state boundaries, safer query construction, better schema guidance, and fewer hidden round trips.

Goal of this document:
- extract the SurrealDB best practices that matter for this crate
- model the library as a finite-state machine at function granularity
- identify which optimizations are immediately safe, which are behavioral, and which are architectural

## Official SurrealDB Best Practices That Matter Here

Researched against official SurrealDB documentation on 2026-03-14.

### 1. Bind values instead of interpolating them

Official guidance:
- SurrealQL parameters are the intended way to supply dynamic values.
- The Rust SDK exposes `.bind(...)` for this purpose.

Why it matters here:
- `Repo<T>` and most graph helpers are mostly aligned with this pattern.
- `RawSql` is intentionally outside that safety boundary and therefore should be treated as an advanced escape hatch.
- `QueryKind::insert_or_replace` still builds identifier fragments dynamically from serialized field names. That is not value interpolation, but it is still a query-construction trust boundary.

Library implication:
- Keep user data in bind variables.
- Treat table names, field names, and relation names as a separate identifier type, not as free `&str` everywhere.

### 2. Prefer explicit schema for stable application data

Official guidance:
- Use `DEFINE TABLE ... SCHEMAFULL` for strongly shaped tables.
- Define field types explicitly with `DEFINE FIELD`.
- Use relation-table features for graph edges, including `TYPE RELATION` and `IN` / `OUT` constraints.

Why it matters here:
- This crate already has schema inventory registration, but it does not actively guide users toward schemafull tables, explicit field definitions, or relation enforcement.
- Generic CRUD without matching schema/index conventions makes performance and correctness depend on undocumented caller behavior.

Library implication:
- The facade should stay generic, but the library should encourage a schema contract: table, fields, relation tables, and indexes.

### 3. Index the access paths you expose

Official guidance:
- Fields used in equality lookups, ordering, and uniqueness constraints should have matching indexes.
- Unique identity beyond the record id should be backed by a unique index.

Why it matters here:
- `find_record_id(k, v)` is only performant if `k` is indexed.
- pagination helpers that order by arbitrary fields assume the caller has defined a compatible index.
- graph lookups by `in` and `out` benefit from relation-table indexing.

Library implication:
- Document index requirements near APIs that depend on them.
- Eventually expose optional schema helpers for common indexes.

### 4. Choose record ids deliberately

Official guidance:
- Record ids are first-class and immutable.
- Range queries over record ids are efficient compared to filter scans when the access pattern matches the key shape.
- If you need synthetic counters, that is an explicit design choice, not a default SurrealDB pattern.

Why it matters here:
- This crate already exposes id-centric CRUD and a dedicated `Id` abstraction.
- `save` is conceptually closer to the SurrealDB model than a secondary-field lookup followed by mutation.

Library implication:
- Keep leaning toward id-addressed APIs.
- Treat secondary lookup helpers as convenience APIs that require matching indexes.

### 5. Use relation tables deliberately

Official guidance:
- `RELATE` and relation tables are the graph-native approach when an edge is a first-class record.
- Relation tables can carry additional payload and should be explicitly defined if they are part of the model.

Why it matters here:
- `GraphRepo` matches the relation-table model, but relation naming is effectively unvalidated and the crate does not enforce relation DDL conventions.

Library implication:
- Add stronger relation identifier validation.
- Encourage `DEFINE TABLE <rel> TYPE RELATION IN <a> OUT <b> ENFORCED` in schema definitions for graph-heavy usage.

### 6. Use explicit transactions only for multi-step atomic work

Official guidance:
- Single statements are already atomic.
- Manual transactions are for multi-step units that must commit or fail together.

Why it matters here:
- `TxRunner` is useful, but its current API drops intermediate results and therefore hides part of the transaction state from callers.

Library implication:
- Keep a transaction API, but return all statement results or a typed transaction report.

### 7. Embedded and local-engine usage should minimize unnecessary write amplification

Official guidance:
- The Rust SDK supports local engines and in-memory modes.
- Performance guidance emphasizes understanding allocator, concurrency, and write-path costs.

Why it matters here:
- `InitDbOptions` exposes versioning, retention, query timeout, transaction timeout, changefeed GC interval, and AST payload.
- Those are valuable knobs, but some of them increase write or storage overhead and should be opt-in, not invisible defaults.

Library implication:
- Current default of non-versioned startup is sensible.
- Large batch APIs should avoid unnecessary extra reads and per-row round trips.

## Source List

- Rust SDK: https://surrealdb.com/docs/sdk/rust
- Parameters: https://surrealdb.com/docs/surrealql/parameters
- Transactions: https://surrealdb.com/docs/surrealql/transactions
- Record IDs: https://surrealdb.com/docs/surrealql/datamodel/ids
- RELATE: https://surrealdb.com/docs/surrealql/statements/relate
- DEFINE TABLE: https://surrealdb.com/docs/surrealql/statements/define/table
- DEFINE FIELD: https://surrealdb.com/docs/surrealql/statements/define/field
- DEFINE INDEX: https://surrealdb.com/docs/surrealql/statements/define/indexes
- Performance best practices: https://surrealdb.com/docs/surrealdb/reference-guide/performance-best-practices

## Library-Wide Conceptual FSM

Global states:
- `G0 Uninitialized`: no database handle, no crypto context, registries may be empty
- `G1 Configured`: options / identifiers / providers prepared but no side effect yet
- `G2 ResourceAcquired`: DB handle, transaction handle, secret store, or registry lock acquired
- `G3 QueryOrPrimitiveBuilt`: SQL, record id, relation id, cipher, or serialized payload assembled
- `G4 Executing`: async DB call, transaction statement, secret-store IO, or crypto operation running
- `G5 Normalizing`: response checked, value decoded, ids normalized, nulls stripped, or fallback path chosen
- `G6 ReadyOrReturned`: stable success result returned
- `GE Failed`: validation, IO, query, check, decode, or not-found error returned

Cross-module pipelines:
- Initialization: `init_db_with_options -> open_db -> use_ns/use_db -> DB.set -> schema inventory replay -> get_db -> Ready`
- CRUD: `caller input -> Repo<T> -> get_db -> build record/query -> execute -> check/take -> typed result`
- Graph: `caller ids -> GraphRepo -> get_db -> relation SQL -> execute/check -> ids or unit`
- Raw SQL: `caller SQL -> RawSql -> get_db -> query -> check/take`
- Transaction: `TxStmt list -> TxRunner -> get_db -> begin -> N statement executions -> commit -> last response`
- Security: `provider/store -> load_or_generate_key -> CryptoContext -> encrypt/decrypt` and `ensure_root_user -> existence check -> signup/signin`

## Function-Level FSM Catalog

Notation:
- `I`: input accepted
- `V`: validate or normalize
- `A`: acquire dependency
- `B`: build SQL, id, config, payload, cipher, or registry state
- `X`: execute side effect
- `N`: normalize or decode output
- `O`: success return
- `E`: error return

### `core/src/connection/mod.rs`

- `InitDbOptions::default`: `I -> B(default non-versioned options) -> O`
- `InitDbOptions::versioned`: `I(self, enabled) -> B(set field) -> O(self)`
- `InitDbOptions::version_retention`: `I -> B(set retention) -> O`
- `InitDbOptions::query_timeout`: `I -> B(set timeout) -> O`
- `InitDbOptions::transaction_timeout`: `I -> B(set timeout) -> O`
- `InitDbOptions::changefeed_gc_interval`: `I -> B(set interval) -> O`
- `InitDbOptions::ast_payload`: `I -> B(set flag) -> O`
- `is_schema_already_defined_error`: `I(message) -> V(lowercase match) -> O(bool)`
- `open_db`: `I(path, options) -> B(Config + SurrealKv builder) -> V(optional versioned/retention branch) -> X(await builder) -> O(db) | E(open/config)`
- `init_db`: `I(path) -> B(default options) -> X(delegate to init_db_with_options) -> O | E`
- `init_db_with_options`: `I(path, options) -> X(create dir) -> X(open_db) -> X(use_ns/use_db) -> X(store global DB) -> A(get_db) -> X(replay schema inventory sequentially) -> N(ignore only already-defined DDL errors) -> O | E(fs/open/use/set/schema)`
- `get_db`: `I -> A(read OnceCell) -> O(Arc<Surreal<Db>>) | E(NotInitialized)`

State observation:
- This module is the single runtime gate. Everything else assumes `G0 -> G6` already happened once.

### `core/src/query/builder.rs`

All functions in this module are pure SQL string emitters. Their FSM shape is `I -> B(assemble SQL template) -> O(String)`.

- `range`: emit record-id range query
- `replace`: emit `UPDATE ... REPLACE` query
- `pagin`: branch on cursor presence and order direction, then emit ordered pagination query
- `rel_pagin`: branch on cursor and order, then emit relation-table pagination query
- `all_by_order`: emit ordered full-table query
- `limit`: emit bounded select query
- `insert`: emit `INSERT IGNORE` query
- `insert_or_replace`: emit `INSERT ... ON DUPLICATE KEY UPDATE` query using caller-supplied field names
- `upsert_set`: emit field-set update query
- `select_id_single`: emit query that returns a single record id by field equality
- `all_id`: emit query returning all ids from a table
- `single_field`: emit query returning one projected field from a table
- `single_field_by_ids`: emit query returning one projected field from a record-id set
- `relate`: emit relation insert query
- `unrelate`: emit edge delete query for a specific pair
- `unrelate_all`: emit edge delete query for all outgoing edges
- `select_out_ids`: emit query returning `out` record ids for an incoming record
- `select_in_ids`: emit query returning `in` record ids for an outgoing record
- `rel_id`: emit query returning a single relation record id
- `create_return_id`: emit create-and-return-id query
- `delete_record`: emit record deletion query
- `delete_table`: emit table wipe query
- `select_by_id`: emit query that projects `record::id(id) AS id`
- `select_all_with_id`: emit full-table select with normalized `id` projection
- `select_limit_with_id`: emit bounded select with normalized `id` projection

State observation:
- The pure-builder model is good for composability.
- The weak point is identifier trust: table names, relation names, and especially `insert_or_replace` field names are not represented by typed safe identifiers.

### `core/src/query/sql.rs`

- `RawSql::query_unchecked`: `I(sql) -> A(get_db) -> X(db.query) -> O(IndexedResults) | E(db/query)`
- `RawSql::query_checked`: `I(sql) -> X(query_unchecked) -> N(response.check) -> O(results) | E(query/check)`
- `RawSql::query_take_typed`: `I(sql, idx) -> X(query_checked) -> N(result.take(idx or 0)) -> O(Vec<T>) | E(check/take)`
- `RawSql::query_return_typed`: `I(sql) -> X(query_checked) -> N(result.take(0)) -> O(Option<T>) | E(check/take)`
- `query_raw`: `I -> X(delegate to RawSql::query_unchecked) -> O | E`
- `query_checked`: `I -> X(delegate to RawSql::query_checked) -> O | E`
- `query_take`: `I -> X(delegate to RawSql::query_take_typed) -> O | E`
- `query_return`: `I -> X(delegate to RawSql::query_return_typed) -> O | E`

State observation:
- This module intentionally bypasses query-shape guarantees. It should stay available, but it should be clearly marked as the unsafe or advanced layer of the facade.

### `core/src/repository/mod.rs` helpers

- `struct_field_names`: `I(data) -> X(serde_json::to_value) -> V(object vs non-object) -> O(Vec<String>) | E(serialize)`
- `strip_null_fields`: `I(value) -> V(recursive object/array walk) -> X(remove null object keys) -> O(mutated value)`
- `extract_record_id_key`: `I(data) -> X(serialize to JSON) -> V(object shape + id field type + non-empty constraints) -> O(RecordIdKey) | E(InvalidModel)`

### `core/src/repository/mod.rs` `Repo<T>` CRUD FSMs

- `create`: `I(model) -> A(get_db) -> X(db.create(table).content(model)) -> N(Option<T> -> T) -> O | E(empty/create)`
- `create_return_id`: `I(model) -> A(get_db) -> B(create_return_id SQL) -> X(query + bind table/data) -> N(check + take id) -> O(RecordId) | E(empty/query)`
- `create_by_id`: `I(id, model) -> A(get_db) -> B(record target) -> X(db.create((table,id)).content) -> N(Option<T> -> T) -> O | E(empty/create)`
- `upsert`: `I(model with HasId) -> A(get_db) -> B(model.id()) -> X(db.upsert(id).content(model)) -> N(Option<T> -> T) -> O | E(empty/upsert)`
- `upsert_by_id`: `I(id, model) -> A(get_db) -> X(db.upsert(id).content(model)) -> N(Option<T> -> T) -> O | E(empty/upsert)`
- `get_by_key`: `I(id key) -> A(get_db) -> X(db.select((table,id))) -> N(Option<T> -> T) -> O | E(NotFound/select)`
- `get_record`: `I(record id) -> A(get_db) -> X(db.select(record)) -> N(Option<T> -> T) -> O | E(NotFound)`
- `scan`: `I -> A(get_db) -> X(db.select(table)) -> O(Vec<T>) | E(select)`
- `select_limit`: `I(count) -> A(get_db) -> B(limit SQL) -> X(query + binds) -> N(check + take rows) -> O(Vec<T>) | E`
- `update_by_id`: `I(id, model) -> A(get_db) -> X(db.update(id).content(model)) -> N(Option<T> -> T) -> O | E(NotFound)`
- `merge`: `I(id, patch object) -> A(get_db) -> X(db.update(id).merge(data)) -> N(Option<T> -> T) -> O | E(NotFound)`
- `patch`: `I(id, ops) -> A(get_db) -> V(empty patch special-case) -> B(chain patch ops) -> X(await final patch query) -> N(Option<T> -> T) -> O | E(NotFound/patch)`
- `insert`: `I(Vec<T>) -> A(get_db) -> X(db.insert(table).content(data)) -> O(Vec<T>) | E(insert)`
- `insert_ignore`: `I(Vec<T>) -> A(get_db) -> V(chunk into 50k) -> loop[B(insert SQL) -> X(query each chunk) -> N(check + take)] -> O(all rows) | E(chunk query)`
- `insert_or_replace`: `I(Vec<T>) -> V(empty fast path) -> A(get_db) -> X(struct_field_names(first row)) -> V(chunk into 50k) -> loop[B(dynamic insert_or_replace SQL) -> X(query each chunk) -> N(check + take)] -> O(all rows) | E`
- `delete_by_key`: `I(id key) -> B(RecordId::new) -> X(delegate delete_record) -> O | E`
- `delete_record`: `I(record id) -> A(get_db) -> B(delete_record SQL) -> X(query + bind) -> N(check) -> O(()) | E`
- `delete_all`: `I -> A(get_db) -> B(delete_table SQL) -> X(query + bind) -> N(ignore only table-missing errors) -> O | E`
- `find_record_id`: `I(field,value) -> A(get_db) -> B(select_id_single SQL) -> X(query + binds) -> N(check + take ids + first) -> O(RecordId) | E(NotFound)`
- `list_record_ids`: `I -> A(get_db) -> B(all_id SQL) -> X(query + bind) -> N(check + take ids) -> O(Vec<RecordId>) | E`
- `save`: `I(model) -> A(get_db) -> X(extract_record_id_key + serialize model) -> V(remove id + strip nulls) -> B(RecordId::new) -> X(db.upsert(record).content(content)) -> N(normalize returned row with id) -> O(T) | E`
- `get`: `I(id key) -> A(get_db) -> B(RecordId::new + select_by_id SQL) -> X(query + bind) -> N(check + take row) -> O(T) | E(NotFound)`
- `list`: `I -> A(get_db) -> B(select_all_with_id SQL) -> X(query + bind table) -> N(check + take rows) -> O(Vec<T>) | E`
- `list_limit`: `I(count) -> A(get_db) -> B(select_limit_with_id SQL) -> X(query + binds) -> N(check + take rows) -> O(Vec<T>) | E`
- `save_many`: `I(Vec<T>) -> V(empty fast path) -> V(chunk into 5k) -> loop[B(compose chunked upsert SQL) -> X(query + binds) -> N(check + normalize rows)] -> O(Vec<T>) | E`

### `core/src/repository/mod.rs` `Crud` trait wrappers

All `Crud` methods are facade delegates. Their FSM is `I -> X(call matching Repo::<Self> function, cloning or deriving id where needed) -> O | E`.

Wrapper mapping:
- `record_id` -> `ModelMeta::record_id`
- `create` -> `Repo::<Self>::create`
- `create_return_id` -> `Repo::<Self>::create_return_id`
- `create_by_id` -> `Repo::<Self>::create_by_id`
- `upsert` -> `Repo::<Self>::upsert`
- `upsert_by_id` -> `Repo::<Self>::upsert_by_id`
- `get_by_key` -> `Repo::<Self>::get_by_key`
- `get_record` -> `Repo::<Self>::get_record`
- `scan` -> `Repo::<Self>::scan`
- `select_limit` -> `Repo::<Self>::select_limit`
- `update` -> `Repo::<Self>::update_by_id(self.id(), self)`
- `update_by_id` -> `Repo::<Self>::update_by_id`
- `merge` -> `Repo::<Self>::merge`
- `patch` -> `Repo::<Self>::patch`
- `insert` -> `Repo::<Self>::insert`
- `insert_ignore` -> `Repo::<Self>::insert_ignore`
- `insert_or_replace` -> `Repo::<Self>::insert_or_replace`
- `delete` -> `Repo::<Self>::delete_record(self.id())`
- `delete_by_key` -> `Repo::<Self>::delete_by_key`
- `delete_record` -> `Repo::<Self>::delete_record`
- `delete_all` -> `Repo::<Self>::delete_all`
- `find_record_id` -> `Repo::<Self>::find_record_id`
- `list_record_ids` -> `Repo::<Self>::list_record_ids`
- `save` -> `Repo::<Self>::save`
- `get` -> `Repo::<Self>::get`
- `save_many` -> `Repo::<Self>::save_many`

State observation:
- The repository layer is the main business-facing capability surface.
- The strongest invariant it assumes is that the caller has already designed matching schema and indexes. That assumption is real but currently implicit.

### `core/src/graph/mod.rs`

- `GraphRepo::relate_by_id`: `I(in_id,out_id,rel) -> A(get_db) -> B(relate SQL) -> X(query + bind rel/in/out) -> N(check) -> O(()) | E`
- `GraphRepo::unrelate_by_id`: `I(self_id,target_id,rel) -> A(get_db) -> B(unrelate SQL) -> X(query + binds) -> N(check) -> O | E`
- `GraphRepo::unrelate_all`: `I(self_id,rel) -> A(get_db) -> B(unrelate_all SQL) -> X(query + binds) -> N(check) -> O | E`
- `GraphRepo::out_ids`: `I(in_id,rel,out_table) -> A(get_db) -> B(select_out_ids SQL) -> X(query + binds) -> N(check + take ids) -> O(Vec<RecordId>) | E`
- `GraphRepo::in_ids`: `I(out_id,rel,in_table) -> A(get_db) -> B(select_in_ids SQL) -> X(query + binds) -> N(check + take ids) -> O(Vec<RecordId>) | E`
- `GraphRepo::insert_relation`: `I(rel, rows) -> A(get_db) -> X(db.insert(rel).relation(rows)) -> O(Vec<Relation>) | E`
- `GraphCrud::relate`: `I(self,target,rel) -> B(self.id,target.id) -> X(GraphRepo::relate_by_id) -> O | E`
- `GraphCrud::unrelate`: `I(self,target,rel) -> B(self.id,target.id) -> X(GraphRepo::unrelate_by_id) -> O | E`
- free `relate_by_id`: `I -> X(GraphRepo::relate_by_id) -> O | E`
- free `unrelate_by_id`: `I -> X(GraphRepo::unrelate_by_id) -> O | E`

State observation:
- Graph capability is thin and good, but it assumes relation names and table names are safe and already schema-backed.

### `core/src/tx/mod.rs`

- `TxStmt::new`: `I(sql) -> B(store sql + empty bindings) -> O(TxStmt)`
- `TxStmt::bind`: `I(stmt,key,val) -> B(convert value to Surreal Value and insert binding) -> O(stmt)`
- `TxRunner::run`: `I(Vec<TxStmt>) -> A(get_db) -> X(begin tx) -> loop[B(tx.query(sql) + apply bindings) -> X(await query) -> N(check response) -> V(store as last_response)] -> X(commit) -> V(empty transaction special-case via RETURN NONE query on db) -> O(IndexedResults) | E(begin/query/check/commit)`
- `run_tx`: `I(stmts) -> X(TxRunner::run) -> O | E`

State observation:
- This is a multi-step FSM, but the current success state collapses all intermediate statement outputs into the last response only.

### `core/src/auth/mod.rs`

- `root_user`: `I(password) -> B(Record<RootCredentials> for namespace=db=app access=account) -> O(record)`
- `ensure_root_user`: `I(password) -> A(get_db) -> X(query_return<bool>(root-exists SQL)) -> V(branch on exists) -> X(sign up or sign in) -> O(()) | E(query/auth)`

State observation:
- This module assumes a particular auth-access shape and checks for existence with a query against the `user` table. That is operationally convenient but brittle if the auth model changes.

### `core/src/model/meta.rs`

- `HasId::id`: abstract contract `I(self) -> O(RecordId)`
- `ModelMeta::table_name`: abstract contract `I(type) -> O(&'static str)`
- `ModelMeta::record_id`: `I(id) -> B(RecordId::new(Self::table_name(), id)) -> O(RecordId)`
- `register_table`: `I(model,table) -> A(lock registry) -> V(reuse existing vs insert new) -> O(&'static str)`
- `default_table_name`: `I(type_name) -> V(strip module path) -> B(to_snake_case + leak boxed str) -> O(&'static str)`
- `to_snake_case`: `I(input) -> V(scan chars and insert underscores before upper-case transitions) -> O(String)`

### `core/src/model/relation.rs`

- `RelationMeta::relation_name`: abstract contract `I(type) -> O(&'static str)`
- `register_relation`: `I(name) -> A(lock registry) -> X(insert into set) -> O(name)`
- `relation_name<R>`: `I(type) -> X(R::relation_name()) -> O(&'static str)`
- `ensure_relation_name`: `I(name) -> O(Ok(()))`

State observation:
- `ensure_relation_name` currently has no validation state. Conceptually it is a stub, not a finished invariant gate.

### `core/src/serde_utils/id.rs`

- `record_key_to_id`: `I(RecordIdKey) -> V(string/number vs other) -> O(Id) | E(String)`
- `Id::as_string`: `I(&Id) -> V(match variant) -> O(Option<&str>)`
- `Id::as_number`: `I(&Id) -> V(match variant) -> O(Option<i64>)`
- `Id::into_record_id_key`: `I(Id) -> B(convert variant) -> O(RecordIdKey)`
- `From<String> for Id`: `I(String) -> O(Id::String)`
- `From<&str> for Id`: `I(&str) -> O(Id::String)`
- `From<i64> for Id`: `I(i64) -> O(Id::Number)`
- `From<Id> for RecordIdKey`: `I(Id) -> X(Id::into_record_id_key) -> O(RecordIdKey)`
- `fmt::Display for Id`: `I(&Id) -> V(match variant) -> O(rendered text)`
- `Serialize for Id`: `I(&Id) -> V(match variant) -> O(serialized primitive)`
- `Deserialize for Id`: `I(deserializer) -> V(parse string/number/record id) -> X(record_key_to_id when needed) -> O(Id) | E`
- `SurrealValue::kind_of`: `I -> O(kind!(string | number))`
- `SurrealValue::is_value`: `I(Value) -> V(match supported primitive or record-id forms) -> O(bool)`
- `SurrealValue::into_value`: `I(Id) -> B(Value::String or Value::Number) -> O(Value)`
- `SurrealValue::from_value`: `I(Value) -> V(match supported forms) -> X(record_key_to_id when needed) -> O(Id) | E`
- `deserialize_id_or_record_id_as_string`: `I(deserializer) -> X(Id::deserialize) -> N(to_string) -> O(String) | E`
- `serialize_id_as_string`: `I(&str) -> O(serialized string)`

### `core/src/crypto.rs`

Traits and providers:
- `KeyProvider::load_key`: abstract contract `I(provider) -> O(Vec<u8>) | E`
- `SecretStore::read_secret`: abstract contract `I(store) -> O(String) | E`
- `SecretStore::write_secret`: abstract contract `I(store,value) -> O(()) | E`
- `KeyBackupStore::read_key`: abstract contract `I(store) -> O(Vec<u8>) | E`
- `KeyBackupStore::write_key`: abstract contract `I(store,value) -> O(()) | E`
- `StaticKeyProvider::new`: `I(key bytes) -> B(store owned bytes) -> O(provider)`
- `StaticKeyProvider::load_key`: `I(provider) -> O(cloned key)`
- `KeyringSecretStore::new`: `I(service,account) -> X(Entry::new) -> O(store) | E(secret-store init)`
- `KeyringSecretStore::read_secret`: `I(store) -> X(entry.get_password) -> N(map_keyring_error) -> O(secret) | E`
- `KeyringSecretStore::write_secret`: `I(store,value) -> X(entry.set_password) -> O | E`
- `KeyringKeyProvider::new`: `I(service,account) -> X(KeyringSecretStore::new) -> B(optional DPAPI backup store) -> O(provider) | E`
- `KeyringKeyProvider::load_key`: `I(provider) -> X(load_or_generate_key) -> O(key) | E`

Crypto context and encryption:
- `CryptoContext::new`: `I(key bytes) -> V(exactly 32 bytes) -> B(copy into fixed array) -> O(context) | E(InvalidKeyLength)`
- `CryptoContext::from_provider`: `I(provider) -> X(provider.load_key) -> X(CryptoContext::new) -> O(context) | E`
- `CryptoContext::cipher`: `I(context) -> B(Aes256Gcm from fixed key) -> O(cipher)`
- `encrypt_bytes`: `I(plaintext,context) -> B(cipher + random nonce) -> X(encrypt) -> N(prefix nonce to ciphertext) -> O(bytes) | E(Encrypt)`
- `decrypt_bytes`: `I(ciphertext,context) -> V(min nonce length) -> B(cipher) -> X(decrypt) -> O(plaintext bytes) | E(CiphertextTooShort/Decrypt)`
- `encrypt_string`: `I(str,context) -> X(encrypt_bytes) -> O(bytes) | E`
- `decrypt_string`: `I(bytes,context) -> X(decrypt_bytes) -> N(UTF-8 decode) -> O(String) | E`
- `encrypt_optional_string`: `I(Option<String>,context) -> V(map Some through encrypt_string) -> O(Option<Vec<u8>>) | E`
- `decrypt_optional_string`: `I(Option<Vec<u8>>,context) -> V(map Some through decrypt_string) -> O(Option<String>) | E`

Helpers and backup path:
- `map_keyring_error`: `I(KeyringError) -> V(NoEntry vs other) -> O(CryptoError)`
- `encode_hex`: `I(bytes) -> B(manual hex encoding) -> O(String)`
- `decode_hex`: `I(str) -> V(expected length) -> loop[X(decode_hex_nibble pairs)] -> O(Vec<u8>) | E(InvalidStoredKey)`
- `decode_hex_nibble`: `I(byte) -> V(hex digit class) -> O(nibble) | E(InvalidStoredKey)`
- `load_or_generate_key`: `I(secret store, optional backup) -> X(read_secret) -> V(existing-secret / backup-restore / fresh-generate branch) -> X(optional store.write_secret) -> X(optional backup.write_key) -> O(key bytes) | E`
- `mirror_key_to_backup`: `I(key,backup?) -> X(best-effort backup write) -> O(())`
- `DpapiKeyBackupStore::new`: `I(service,account) -> X(fallback_key_path) -> O(Option<store>)`
- `DpapiKeyBackupStore::read_key`: `I(store) -> V(path exists) -> X(fs::read) -> X(unprotect_backup_bytes) -> O(key) | E`
- `DpapiKeyBackupStore::write_key`: `I(store,value) -> X(create parent dirs) -> X(protect_backup_bytes) -> X(fs::write) -> O | E`
- `fallback_key_path`: `I(service,account) -> V(read LOCALAPPDATA) -> X(sanitize_path_component twice) -> B(path join) -> O(Option<PathBuf>)`
- `sanitize_path_component`: `I(str) -> V(map non `[A-Za-z0-9_-]` to `_`) -> O(String)`
- `protect_backup_bytes` on Windows: `I(bytes) -> X(CryptProtectData) -> N(copy protected blob + LocalFree) -> O(bytes) | E`
- `unprotect_backup_bytes` on Windows: `I(bytes) -> X(CryptUnprotectData) -> N(copy decrypted blob + LocalFree) -> O(bytes) | E`
- `protect_backup_bytes` on non-Windows: `I(bytes) -> O(cloned bytes)`
- `unprotect_backup_bytes` on non-Windows: `I(bytes) -> O(cloned bytes)`

## What Is Already a Good Design Choice

- Capability layering plus facade export is the correct direction for this crate.
- `ModelMeta`, `HasId`, `Crud`, `GraphCrud`, and the `prelude` surface give callers a coherent API without forcing domain architecture onto them.
- `Id` as a string-or-number bridge is useful because it aligns the Rust model with actual SurrealDB record-id variants.
- The crypto module already separates provider, secret store, backup store, and cipher context. That separation is good and extensible.

## Optimization And Risk Review

### P0: correctness and safety issues

- `core/src/model/relation.rs`: `ensure_relation_name` is a no-op. The crate conceptually has a relation-identifier invariant, but it is not enforced anywhere.
- `core/src/query/sql.rs`: `RawSql` exposes unchecked raw SQL without a paired safe builder-and-bind API. That is fine as an escape hatch, but the facade should make the trust boundary explicit.
- `core/src/repository/mod.rs`: `insert_or_replace` trusts serialized field names when constructing SQL. Even if model field names are usually compile-time, this is still identifier construction without validation.
- `core/src/auth/mod.rs`: `ensure_root_user` assumes access `account`, namespace `app`, database `app`, and a queryable `user` table shape. That is a brittle operational contract.
- `core/src/tx/mod.rs`: transaction success returns only the last statement response. Intermediate state exists but is discarded, which makes it harder to reason about correctness for multi-step flows.

### P1: obvious performance issues

- `core/src/repository/mod.rs`: older per-row `insert_jump_by_id_value` style batching would be `O(n)` network or engine round trips. The current `save_many` design avoids that by composing chunked upserts into a single query per batch.
- `core/src/repository/mod.rs`: older `upsert_by_id_value` style flows needed a follow-up select after write. The current `save` path normalizes the returned row directly and avoids that extra read.
- `core/src/repository/mod.rs`: `scan` is still a footgun for large tables. It is convenient, but it makes an unbounded full read easy to reach.
- `core/src/connection/mod.rs`: schema inventory DDL is replayed sequentially on every initialization and idempotence is inferred by string matching on error messages.

### P2: architectural limitations

- `core/src/connection/mod.rs`: the global `OnceCell` singleton makes multi-database use, test isolation, and dependency injection harder than necessary.
- `core/src/query/builder.rs`: query construction currently mixes safe placeholders with untyped identifier strings. The next abstraction boundary should be typed identifiers, not more string helpers.
- `core/src/graph/mod.rs`: graph helpers assume relation schema exists but do not participate in relation-schema definition or validation.
- `core/src/model/meta.rs`: `default_table_name` leaks strings permanently. For macro-derived metadata this is acceptable, but it is still a deliberate global-allocation tradeoff.

## Recommended Next Refactor Order

### Step 1: harden invariants without breaking the public facade

- Implement real identifier validation for relation names and, if kept as free strings, for table and field identifiers used in SQL builders.
- Remove library `println!` calls and replace them with optional progress callbacks or leave progress reporting to callers.
- Mark `RawSql` as advanced or unsafe-by-convention in documentation and add a `query_bound(sql, bindings)` API for the common safe raw-query case.

### Step 2: remove avoidable round trips

- Keep `save_many` on chunked batch upserts rather than regressing to per-row write loops.
- Keep `save` on a single write path that returns the normalized typed row directly.
- Consider a bounded iterator or page-stream API instead of treating `scan` as the normal convenience path.

### Step 3: make runtime state explicit

- Introduce a `DbRuntime` or similar capability object that wraps the `Surreal<Db>` handle and schema state.
- Keep the current global facade as a convenience layer built on top of that object, rather than as the only runtime model.
- Rework `TxRunner::run` to return all statement responses, or a typed transaction report with per-statement success data.

### Step 4: align schema utilities with SurrealDB best practice

- Add optional helpers or macros for `SCHEMAFULL` tables, field definitions, relation-table definitions, and common indexes.
- Document which repository methods require matching unique indexes or order indexes to remain performant.

## Recommended Conceptual Model Going Forward

The right architecture is not DDD-first. It is:

- capability layer: connection, schema, query plan, transaction, repository, graph, crypto
- invariant layer: typed ids, typed identifiers, schema expectations, relation expectations
- facade layer: `appdb::...` exports that remain simple for users

In FSM terms, the library should move from implicit state transitions to explicit capability transitions:

- current model: many functions assume global readiness
- target model: capability objects prove readiness and restrict which transitions are legal

That keeps the API simple for `cargo add appdb; use appdb::Sensitive; use appdb::other_fn;`, while making internals safer and easier to optimize.
