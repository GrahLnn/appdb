# appdb

`appdb` 是一个给 Tauri 嵌入式 SurrealDB 使用的轻量辅助库。

目标只有几个：

- 用 `#[derive(Store)]` 给模型挂上直接可用的仓储能力
- 用 prelude 导出一个简单的使用面
- 保持 API 以 `save`、`get`、`list` 这种直观名字为主
- 给需要加密的字段提供 `Sensitive` 派生支持

## 工作区结构

- `core`: 主库，发布后通过 `cargo add appdb` 使用
- `macros`: 过程宏，供 `core` 复用并一并导出

## 常用入口

- `appdb::prelude::*`: 常用类型和能力的集中导出
- `appdb::connection`: 数据库初始化和运行时
- `#[derive(Store)]`: 业务模型的主入口
- `appdb::graph::GraphRepo`: relation table 辅助
- `appdb::query`: 原始 SQL 与带 bind 的查询辅助

## 最小示例

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

## `Store` + `Sensitive` 联动

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

对这种模型，业务代码仍然直接使用 `Profile` 调 `save` / `get` / `list` 等 Store API；`#[secure]` 字段会在仓储边界自动加密落库、读回时自动解密。第一版里 `create_return_id` 不支持敏感模型，且 `#[secure]` 字段不能参与 `#[unique]` 或自动 lookup。

## `#[store(ref)]` 嵌套引用

`Store` 现在支持显式嵌套引用字段：

```rust
use appdb::prelude::*;
use appdb::Store;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Child {
    #[unique]
    code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, Store)]
struct Parent {
    id: Id,
    #[store(ref)]
    child: Child,
}
```

当前语义：

- 只有显式标注 `#[store(ref)]` 的字段才走嵌套引用路径
- 首版支持 `Child`、`Option<Child>`、`Vec<Child>`
- 保存父对象时，会先按子对象的 `id` 或现有 lookup 规则解析子记录；找不到时自动创建子记录
- 父表里只保存子记录的 `RecordId`（或其数组），读取父对象时会自动 hydrate 回完整子对象
- 这一层不会扩展到 `merge` / `patch` / 原始 query helper，也不承诺父子写入事务原子性

## 图关系示例

```rust
use appdb::prelude::*;

let rel = relation_name::<FollowRel>();
GraphRepo::relate_at(user_a.id(), user_b.id(), rel).await?;
let targets = GraphRepo::out_ids(user_a.id(), rel, "user").await?;
```

## 原始查询

优先用带 bind 的形式：

```rust
use appdb::prelude::*;

let stmt = RawSqlStmt::new("RETURN $value;").bind("value", 42);
let value: Option<i64> = query_bound_return(stmt).await?;
```

## 说明

- 更细的行为说明已经写进源码里的 rustdoc，直接看对应函数和结构体即可。
- 这个库偏向单机嵌入式使用场景，不追求大而全的抽象层。
