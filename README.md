# appdb

`appdb` is a lightweight SurrealDB helper library for embedded applications, including Tauri-style desktop apps. It provides derive-driven model APIs, a small public surface, and optional field encryption for local-first persistence.

The workspace publishes two crates:

- `appdb`: the main library
- `appdb-macros`: procedural macros used by `appdb`

## Installation

```bash
cargo add appdb
```

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

    let saved = User::save(User {
        id: Id::from("u1"),
        name: "alice".into(),
    })
    .await?;

    let loaded = User::get("u1").await?;
    let all = User::list().await?;

    assert_eq!(saved.name, loaded.name);
    assert_eq!(all.len(), 1);
    Ok(())
}
```

## Core Concepts

### Model-first CRUD

`#[derive(Store)]` generates model-level persistence APIs such as `save`, `save_many`, `create`, `get`, and `list`. The intended public API is the model type itself rather than manually assembling repository calls.

Common imports are re-exported from `appdb::prelude::*`.

### Managed schema startup and schemaless persistence

`init_db*` and `DbRuntime::open*` are the schema-managed startup path. They apply registered schema items such as indexes generated from `#[unique]`.

Persistence itself keeps a separate contract: first saves on the default embedded runtime still support schemaless storage. Startup management and model CRUD are related, but they are not the same guarantee.

### Sensitive fields

`#[derive(Sensitive)]` supports encrypted fields marked with `#[secure]`.

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
}
```

The model still uses the same `Store` APIs, while secure fields are encrypted before persistence and decrypted on read.

Sensitive models now auto-register their crypto metadata on first runtime use, so the default `Store`/resolver paths do not require manual registration code. You can override the defaults globally with `appdb::crypto::set_default_crypto_service`, `set_default_crypto_account`, or `set_default_crypto_config`, and refine a model or field with `#[crypto(...)]`.

Supported secure shapes include:

- `String`
- `Option<String>`
- nested `Sensitive` children such as `Child`, `Option<Child>`, and `Vec<Child>`
- enum-bearing leaves inside a secure container via `SensitiveValueOf<T>`

Every `Sensitive` model also exposes stable secure-field metadata through `Model::secure_fields()`.

### Foreign fields

Use `#[foreign]` on supported child model fields to persist related values as record links while hydrating them back into full models when reading.

Supported shapes include:

- `Child`
- `Option<Child>`
- `Vec<Child>`

`#[table_as(...)]` is also supported for referenced models.

### Graph relations

`GraphRepo` provides helpers around SurrealDB relation tables.

```rust
use appdb::prelude::*;

let rel = relation_name::<FollowRel>();
GraphRepo::relate_at(user_a.id(), user_b.id(), rel).await?;
let targets = GraphRepo::out_ids(user_a.id(), rel, "user").await?;
```

### Raw SQL with bind values

For queries outside the derive-driven CRUD surface, use the raw SQL helpers with bind values.

```rust
use appdb::prelude::*;

let stmt = RawSqlStmt::new("RETURN $value;").bind("value", 42);
let value: Option<i64> = query_bound_return(stmt).await?;
```

## Capabilities

- `#[derive(Store)]` for model-level CRUD
- `appdb::prelude::*` for common imports
- `#[derive(Sensitive)]` and `#[secure]` for encrypted fields
- `#[unique]`-driven schema registration
- Foreign fields via `#[foreign]`
- Table remapping with `#[table_as(...)]`
- Graph relation helpers via `GraphRepo`
- Raw SQL helpers with bind support

## Workspace Layout

- `core/`: source for the published `appdb` crate
- `macros/`: source for the published `appdb-macros` crate

## Development

Run the standard Rust 2024 workspace validators from the workspace root:

```bash
cargo check --workspace --all-targets
cargo test --workspace -- --test-threads 12
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
