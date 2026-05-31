# appdb

[![Crates.io](https://img.shields.io/crates/v/appdb.svg)](https://crates.io/crates/appdb)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/GrahLnn/appdb/actions/workflows/ci.yml/badge.svg)](https://github.com/GrahLnn/appdb/actions/workflows/ci.yml)

`appdb` is a lightweight SurrealDB helper library for embedded Rust applications,
especially local-first desktop apps. It gives domain models a small, derive-driven
API for persistence, typed projections, graph relations, encrypted fields,
schema registration, raw SurrealQL, and explicit transaction work.

The workspace publishes two crates:

- `appdb`: the runtime, model APIs, query helpers, and public re-exports
- `appdb-macros`: the procedural macros re-exported by `appdb`

The current workspace targets Rust 2024 and requires Rust `1.94.0` or newer.

## Installation

Application crates usually need `appdb`, `serde`, `surrealdb`, `tokio`, and an
error type such as `anyhow`:

```bash
cargo add appdb
cargo add serde --features derive
cargo add surrealdb@3.1.2 --features kv-surrealkv
cargo add tokio --features macros,rt-multi-thread
cargo add anyhow
```

`appdb` re-exports its derive macros, so application code can import `Store`,
`View`, `Sensitive`, `Bridge`, and `Relation` from `appdb`.

## Quick Start

```rust
use appdb::prelude::*;
use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct User {
    id: Id,
    name: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_db("data/appdb".into()).await?;

    let saved = User {
        id: Id::from("u1"),
        name: "alice".to_owned(),
    }
    .save()
    .await?;

    let loaded = User::get("u1").await?;
    let all = User::list().await?;

    assert_eq!(saved.name, loaded.name);
    assert_eq!(all.len(), 1);
    Ok(())
}
```

The generated model methods are the intended application-facing API. `Repo<T>`
remains public for advanced integration seams, but normal application code should
stay on the model type.

## Runtime

`init_db(path)` opens an embedded SurrealKV database, selects the `app/app`
namespace and database, applies registered schema items, and installs the handle
used by model, graph, view, query, and transaction helpers.

Use `init_db_with_options(path, InitDbOptions::default()...)` when the database
needs SurrealKV versioning, retention, query timeouts, transaction timeouts,
changefeed garbage collection, or AST payload storage.

Use `DbRuntime::open*` when a caller needs to own a runtime and install it later
with `DbRuntime::install_global()`.

## Store Models

`#[derive(Store)]` turns a struct with named fields into a persisted model. It
generates table metadata, id helpers, stored-shape conversion, lookup metadata,
and model-level methods such as:

- `save`, `save_many`, `get`, `get_record`
- `list`, `list_limit`, `list().order_by(...)`
- `create_at`, `upsert_at`, `update_at`
- `delete`, `delete_all`, `exists`
- `find_one_id`, `list_record_ids`

```rust
use appdb::prelude::*;
use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[unique]
    slug: String,
    #[pagin]
    created_at: i64,
    title: String,
}

async fn example() -> anyhow::Result<()> {
    let page = Post::pagin_desc(20, None).await?;
    let ordered = Post::list().order_by("created_at", Order::Desc).await?;
    let post_id = Post::find_one_id("slug", "hello").await?;
    Ok(())
}
```

`#[unique]` registers schema indexes for automatic lookup. appdb can resolve a
record from the declared lookup fields, including foreign-backed lookup fields
that first resolve to child `RecordId` values.

`#[pagin]` registers a stable keyset-pagination index and enables `pagin_desc` /
`pagin_asc`, which return `Page<T>` with `items` and an optional `PageCursor`.
Ordering a full list is limited to `id` and the declared pagination field, which
keeps list ordering explicit instead of accepting arbitrary field names.

Explicit-id writes use full `RecordId` values. `create_at` fails on conflict,
while `upsert_at` and `save` update the same addressed record.

## Auto-filled Fields

`AutoFill` is an appdb-managed scalar for fields that should be filled by the
write path. Application code passes `AutoFill::pending()` instead of manually
writing the value. The Store write path resolves pending values before
persistence and returns the resolved value in the saved model.

Today the supported fill policy is `#[fill(now)]`. It writes the current UTC
timestamp as a string, normalized with a fixed nine-digit fractional second:
`YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`. It is not an integer millisecond timestamp.

```rust
use appdb::prelude::*;
use appdb::{AutoFill, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Entry {
    id: Id,
    #[pagin]
    #[fill(now)]
    created_at: AutoFill,
    title: String,
}

async fn example() -> anyhow::Result<()> {
    let saved = Entry {
        id: Id::from("entry-1"),
        created_at: AutoFill::pending(),
        title: "created by appdb".to_owned(),
    }
    .save()
    .await?;
    assert!(!saved.created_at.is_pending());
    Ok(())
}
```

Pending values are resolved on `save` and `save_many`; already resolved values
are preserved. Use `AutoFill::resolved(value)` only when importing or replaying a
known timestamp that should not be replaced.

## Foreign Fields

Use `#[foreign]` when a model field should store record links but hydrate back
into full caller-facing values.

```rust
use appdb::prelude::*;
use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Author {
    id: Id,
    #[unique]
    handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Article {
    id: Id,
    title: String,
    #[foreign]
    authors: Vec<Author>,
}
```

Supported shapes are recursive `Option<_>` and `Vec<_>` wrappers whose leaf type
implements appdb's foreign bridge. Store models implement that bridge
automatically. Use `#[derive(Bridge)]` on an enum when one foreign field can point
to multiple Store model types.

`#[table_as(Target)]` lets an alias model reuse another Store model's table and
lookup metadata. That is useful when a caller needs a narrower Rust shape over an
existing persisted table.

For explicit-id writes where related records already exist, call
`model.foreign().field_name(record_id_shape).upsert_at(...)`. The generated
foreign-write builder can also return a `View` directly with
`create_at_returning::<View>`, `upsert_at_returning::<View>`, and
`update_at_returning::<View>`.

## Relation Fields And Graph Helpers

Use `#[relate("edge_table")]` or `#[back_relate("edge_table")]` when a field
should live in a SurrealDB relation table instead of being stored inline on the
parent row.

```rust
use appdb::prelude::*;
use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Tag {
    id: Id,
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Note {
    id: Id,
    title: String,
    #[relate("note_tags")]
    tags: Vec<Tag>,
}
```

Relation-backed fields support direct, `Option<T>`, `Vec<T>`, and
`Option<Vec<T>>` shapes. `save`, `save_many`, `create`, `get`, `list`, and
`list_limit` synchronize and hydrate those relation fields.

For direct graph work, use `#[derive(Relation)]`, `GraphRepo`, the free graph
functions, or generated model methods such as `relate_by_name`,
`back_relate_by_name`, `unrelate_by_name`, `outgoing_ids`, `incoming_ids`,
`outgoing`, `incoming`, `outgoing_count`, `incoming_count`,
`outgoing_count_as`, and `incoming_count_as`.

```rust
use appdb::{Relation, Store};

#[derive(Debug, Clone, Copy, Relation)]
#[relation(name = "note_links")]
struct NoteLinks;
```

## Read-only Views

`#[derive(View)]` defines a typed read projection. A table-backed View reads only
its declared fields from a Store source, so callers can expose list or detail
surfaces without loading every field from the source model.

```rust
use appdb::prelude::*;
use appdb::{Store, View};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Post {
    id: Id,
    #[pagin]
    created_at: i64,
    title: String,
    body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, View)]
#[view(source = Post)]
struct PostListItem {
    id: Id,
    title: String,
}

async fn example() -> anyhow::Result<()> {
    let items = PostListItem::list()
        .order_by("created_at", Order::Desc)
        .await?;
    let one = PostListItem::get("post-1").await?;
    Ok(())
}
```

View list ordering accepts `id`, declared View fields, and the source model's
`#[pagin]` field even when that field is not exposed by the View.

Nested Views are declared with `#[view(nested)]` and can use direct, `Option<_>`,
`Vec<_>`, or recursive `Option` / `Vec` wrappers:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, View)]
#[view(source = Article)]
struct ArticleConfig {
    id: Id,
    title: String,
    #[view(nested)]
    authors: Vec<AuthorListItem>,
}
```

Table-backed Views expose `list`, `list().order_by(...)`, `get`, `get_record`,
`find_one`, `find_one_id`, `list_records`, `outgoing_records`,
`incoming_records`, and batch relation queries that preserve each owner record.

## SQL-backed Views

Views can also be backed by a custom SurrealQL statement. SQL-backed Views use
typed parameters and are queried through `View::query(params)`.

```rust
use appdb::model::meta::{ModelMeta, ViewParams};
use appdb::query::RawSqlStmt;
use appdb::prelude::*;
use appdb::View;
use serde::{Deserialize, Serialize};
use surrealdb::types::{SurrealValue, Table};

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, View)]
#[view(
    sql = "SELECT record::id(id) AS id, created_at, title FROM $table WHERE created_at <= $before ORDER BY created_at DESC, id DESC LIMIT $limit;",
    params = RecentPosts
)]
struct RecentPost {
    id: Id,
    created_at: i64,
    title: String,
}

struct RecentPosts {
    before: i64,
    limit: i64,
}

impl ViewParams for RecentPosts {
    fn bind_view_params(self, stmt: RawSqlStmt) -> anyhow::Result<RawSqlStmt> {
        Ok(stmt
            .bind("table", Table::from(Post::storage_table()))
            .bind("before", self.before)
            .bind("limit", self.limit))
    }
}

async fn example() -> anyhow::Result<()> {
    let recent = RecentPost::query(RecentPosts {
        before: 1_900_000_000,
        limit: 20,
    })
    .await?;
    Ok(())
}
```

SQL-backed Views do not support table-source methods such as `list()` and
`get()`, because their source is the custom query itself.

## Sensitive Fields

`#[derive(Sensitive)]` encrypts fields marked with `#[secure]` before
persistence and decrypts them on read.

```rust
use appdb::prelude::*;
use appdb::{Sensitive, Store};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store, Sensitive)]
struct Profile {
    id: Id,
    alias: String,
    #[secure]
    secret: String,
    #[secure]
    note: Option<String>,
}
```

Supported secure shapes include `String`, `Option<String>`, nested
`Sensitive` children, `Option<Child>`, `Vec<Child>`, and
`SensitiveValueOf<T>` for enum-bearing payloads inside an approved secure
container.

Sensitive models auto-register crypto metadata on first runtime use. Override
defaults globally with `set_default_crypto_service`,
`set_default_crypto_account`, or `set_default_crypto_config`, and refine models
or fields with `#[crypto(...)]`.

By default, auto-registered crypto contexts use the OS keyring and a local
protected backup for the symmetric key. Secure field ciphertext is therefore
bound to the machine/user environment that generated that key. Copying the
database to another machine does not make `#[secure]` fields decryptable there;
only non-secure fields remain portable by themselves. Cross-machine decryption
requires explicitly registering a crypto context backed by the same externally
managed key.

## Schema And Vector Indexes

`#[unique]`, `#[pagin]`, `impl_schema!`, and `impl_hnsw_index!` register schema
items through inventory. Runtime initialization applies those items
idempotently.

```rust
use appdb::model::schema::{VectorDistance, VectorIndexType};

struct EventEmbedding;

appdb::impl_hnsw_index!(
    EventEmbedding,
    name: "event_embedding_hnsw",
    table: "event_embedding",
    field: "embedding",
    dimension: 64,
    vector_type: VectorIndexType::F32,
    distance: VectorDistance::Cosine,
    ef_construction: 150,
    m: 12,
    concurrently: true,
);
```

HNSW index definitions validate plain identifiers, support nested field paths
such as `items.embedding`, and render SurrealDB `DEFINE INDEX ... HNSW` DDL.

## Raw SQL And Transactions

For query shapes outside the derive-driven API, use `RawSqlStmt` and the raw
query helpers. Unbound helpers include `query_raw`, `query_checked`,
`query_take`, and `query_return`; bound helpers include `query_bound`,
`query_bound_checked`, `query_bound_take`, and `query_bound_return`.

```rust
use appdb::prelude::*;

async fn example() -> anyhow::Result<()> {
    let stmt = RawSqlStmt::new("RETURN $value;").bind("value", 42);
    let value: Option<i64> = query_bound_return(stmt).await?;
    Ok(())
}
```

Use `run_tx` and `TxStmt` when several statements must run inside one explicit
SurrealDB transaction.

Errors are normalized through `DBError` / `DBErrorKind` for common cases such as
not found, missing table, conflict, decode failure, invalid models, and query
response errors.

## Workspace Layout

- `core/`: source for the published `appdb` crate
- `macros/`: source for the published `appdb-macros` crate

## Development

Run the workspace checks from the repository root:

```bash
cargo check --workspace --all-targets
cargo test --workspace -- --test-threads 12
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
