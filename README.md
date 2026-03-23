# appdb

`appdb` 是一个给 Tauri 嵌入式 SurrealDB 使用的轻量辅助库。

目标只有几个：

- 用 `#[derive(Store)]` 给模型挂上直接可用的模型级 CRUD 能力
- 用 prelude 导出一个简单的使用面
- 保持推荐 API 以模型自身的 `save`、`get`、`list` 这种直观名字为主
- 给需要加密的字段提供 `Sensitive` 派生支持

## 工作区结构

- `core`: 主库，发布后通过 `cargo add appdb` 使用
- `macros`: 过程宏，供 `core` 复用并一并导出

## 常用入口

- `appdb::prelude::*`: 常用类型和能力的集中导出
- `appdb::connection`: 数据库初始化和运行时
- `#[derive(Store)]`: 业务模型的主入口，推荐直接通过模型类型调用 CRUD
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

## 推荐 public API vs internal helpers

推荐给业务调用方的主路径：

- 在模型上直接调用 `save` / `save_many` / `create` / `get` / `list`
- 需要实例方法时，通过 `appdb::Crud` trait 提供的包装调用
- 图关系继续使用 `GraphRepo` / relation helpers

不推荐把下面这些当成日常业务入口：

- `appdb::repository::Repo::<T>`：这是仓储内部构建层，主要给库内部、测试和少量高级集成 seam 使用
- 直接围绕 internal helper 组合 public CRUD 流程

换句话说，`Repo` 仍然公开以保留扩展能力，但文档约定的主 API 已经收敛到模型级 Store/Crud surface，而不是 `Repo::<T>` 组合层。

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
- `save` / `get` / `get_record` / `list` / `list_limit` 会保持一致的 hydrate 结果
- 失败的 `save` / `save_many` 不应留下父子半成功残留；成功返回的对象应与后续读取结果一致
- 原始 query 读路径支持字符串形态的 record link（例如 ``child:`c1```）回到正常 hydrate 流程
- `#[table_as(...)]` 模型参与嵌套引用时，仍通过目标表落库并保持同样的 roundtrip 语义
- 这一层不会扩展到 `merge` / `patch`

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

- `DbRuntime::open*` / `init_db*` 走的是 **schema-managed** 启动路径：运行时会先应用通过 schema inventory 注册的 DDL（例如 `#[unique]` 生成的索引）。
- 直接用默认的嵌入式运行时做首次 `save` / `upsert_at` 时，库仍然保证 **schemaless** 持久化可用；这条承诺是独立的，不能把 managed 启动时顺带应用的 schema side effects 当作它的证明。
- 因此文档里的推荐调用顺序是：把 `init_db*` / `DbRuntime::open*` 视为 managed schema 启动入口；把模型级 `save` / `get` / `list` 视为稳定的 public CRUD surface。两者分别表达启动契约与持久化契约，不要混成“必须先走 internal repo helper 才能正确保存”。
- 更细的行为说明已经写进源码里的 rustdoc，直接看对应函数和结构体即可。
- 这个库偏向单机嵌入式使用场景，不追求大而全的抽象层。
