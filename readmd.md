# appdb

`appdb` 是一个给 Tauri 嵌入式 SurrealDB 使用的轻量辅助库。

目标只有几个：

- 用 `impl_crud!` 给模型挂上直接可用的仓储能力
- 用 facade / prelude 导出一个简单的使用面
- 保持 API 以 `save`、`get`、`list` 这种直观名字为主
- 给需要加密的字段提供 `Sensitive` 派生支持

## 工作区结构

- `core`: 主库，发布后通过 `cargo add appdb` 使用
- `macros`: 过程宏，供 `core` 复用并一并导出

## 常用入口

- `appdb::prelude::*`: 常用类型和能力的集中导出
- `appdb::connection`: 数据库初始化和运行时
- `appdb::repository::Repo`: 直接用类型驱动的 CRUD
- `appdb::graph::GraphRepo`: relation table 辅助
- `appdb::query`: 原始 SQL 与带 bind 的查询辅助

## 最小示例

```rust
use appdb::prelude::*;
use appdb::{impl_crud, impl_id};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct User {
    id: String,
    name: String,
}

impl_crud!(User, "user");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_db("data/appdb".into()).await?;

    let saved = Repo::<User>::save(User {
        id: "u1".into(),
        name: "alice".into(),
    })
    .await?;

    let loaded = Repo::<User>::get("u1").await?;
    let all = Repo::<User>::list().await?;

    assert_eq!(saved.name, loaded.name);
    assert_eq!(all.len(), 1);
    Ok(())
}
```

## 图关系示例

```rust
use appdb::prelude::*;

let rel = relation_name::<FollowRel>();
GraphRepo::relate_by_id(user_a.id.clone(), user_b.id.clone(), rel).await?;
let targets = GraphRepo::out_ids(user_a.id.clone(), rel, "user").await?;
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
