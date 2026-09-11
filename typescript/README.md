# appdb TypeScript

This directory contains the native TypeScript implementation of the appdb
model, query, repository, graph, pagination, connection, and crypto APIs. It
uses Effect `4.0.0-rc.115` for services, layers, scopes, schemas, and typed
expected errors. The package is a private workspace artifact while its storage
consumers are completed.

The supported toolchain is Node `>=22.12.0`. The package currently resolves
the following database packages:

| Package | Version | Boundary |
| --- | --- | --- |
| `surrealdb` | `2.0.8` (patched locally) | TypeScript SDK and remote engines |
| `@surrealdb/node` | `3.0.3` | Node native-engine adapter |
| embedded engine metadata | `3.0.2` | Version reported by the native package metadata |
| Rust workspace `surrealdb` | `3.2.4` | Separate Rust implementation; it is not the TypeScript engine |

The installed native package reports embedded engine version `3.0.2` at
runtime; this is distinct from the `@surrealdb/node` package version `3.0.3`.
The embedded engine version is a runtime property of the installed native
package, and the metadata above is not a substitute for a runtime probe. The Rust
workspace's `surrealdb` `3.2.4` dependency is a separate local implementation;
it does not establish compatibility with a remote SurrealDB `3.2.4` server, and
this package makes no such remote-version claim.

## Entry points

`@appdb/core` is the shared, engine-agnostic entry point. It exports the Effect
`Database` and `Crypto` services, connection layers, typed errors, IDs, models,
schema metadata, query and transaction helpers, repositories, graph helpers,
relations, and pagination. It does not statically import the Node native-engine
adapter or the operating-system keyring addon.

`@appdb/core/node` is the Node-only entry point. It exports
`makeNodeDatabaseLayer` (also `NodeDatabaseLayer`), `nodeEndpoint`, the
embedded-endpoint types, and the Node keyring provider and layer. Import this
entry point only from Node code:

```ts
import { Effect, Schema } from "effect"
import { Model, RootIdSchema, field } from "@appdb/core"
import { makeNodeDatabaseLayer, nodeEndpoint } from "@appdb/core/node"

const UserSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  email: field(Schema.String, { unique: true }),
})
const User = Model.define("user", UserSchema)

const nodeLayer = makeNodeDatabaseLayer({
  endpoint: nodeEndpoint("mem"),
  namespace: "app",
  database: "app",
})

const result = await Effect.runPromise(
  Effect.gen(function* () {
    const decoded = yield* User.decode({ id: "alice", email: "alice@example.com" })
    return yield* User.encode(decoded)
  }),
)

console.log({ result, nodeLayerDefined: nodeLayer !== undefined })
```

Constructing a Node layer is lazy. The example creates a layer but does not
open an engine or require a local database; an application starts it by
providing the layer to an Effect runtime.

## Public API

The root entry point is the package boundary. The source files under `src/`
are implementation modules and are not public import paths.

### Models and schemas

- `Model.define(table, schema)` (also exported as `define`) creates a store
  model, `Model.view(owner, schema, options)` (also exported as `view`) creates
  a view projection over an owner model, and `Model.sqlView(name, schema,
  options)` creates a standalone SQL view model.
- `field(schema, metadata)` attaches storage metadata without changing the
  decoded or encoded value type. `foreign(target, options)` and
  `relation(target, options)` build lazy reference fields. `compileSchemaPlan`
  and `fieldSchemaOf` expose the metadata compilation boundary.
- `foreign` and `relation` use `Schema.suspend` around the target model. The
  target thunk is not evaluated while the parent model is defined, and a
  foreign or relation boundary keeps the target's `id`, `unique`, and
  sensitive metadata out of the parent's root plan. Cardinality is
  `one`, `nullable`, `many`, or `nullable-many`. When the parent field carries
  the exact target schema, the repository reuses that encoded value at the
  child boundary instead of applying the target codec twice.
- Metadata supports `id`, `unique`, `pagination`, `foreign`, `relate`, and
  `sensitive`. The reference cardinalities are `one`, `nullable`, `many`, and
  `nullable-many`.
- `RootIdSchema`, `RootIntSchema`, `I64Schema`, `I64_MIN`, `I64_MAX`,
  `makeRecordId`, `rootIdFromRecord`, `assertRootId`, and the ID predicates
  provide the shared root-ID and `RecordId` boundary.

`Model.decode` and `Model.encode` return Effects. Decode and encode failures are
mapped to the shared `DbError` family. Foreign and relation targets remain
lazy: model definition and schema planning do not hydrate a target or perform
database I/O.

An encode-side default can use the native Effect Schema codec API. Keep the
decoded input optional so `undefined` is the pending state, and generate the
value only while encoding for a write:

```ts
import { Clock, Effect, Schema, SchemaGetter } from "effect"
import { Model, RootIdSchema, field } from "@appdb/core"

const pendingOrResolved = Schema.Union([Schema.String, Schema.Undefined])
const FillNow = pendingOrResolved.pipe(
  Schema.decodeTo(pendingOrResolved, {
    decode: SchemaGetter.transformEffect<string | undefined, string | undefined>(
      (value) => Effect.succeed(value),
    ),
    encode: SchemaGetter.withDefault(
      Effect.map(Clock.currentTimeMillis, (ms) => new Date(ms).toISOString()),
    ),
  }),
)

const Event = Model.define(
  "event",
  Schema.Struct({
    id: field(RootIdSchema, { id: true }),
    createdAt: FillNow,
  }),
)
```

Here `undefined` is an application-level pending value, not a database default.
The codec fills it once on the write path; a stored timestamp decodes without
running the fill again. The Effect `Clock` service also keeps the source of
time controllable in tests.

### Schema startup

`SchemaDdlDefinition` groups optional `rawDdl`, `models`, and explicit `hnsw`
definitions. `schemaDdl` preserves raw statements byte-for-byte and in their
given order, then appends generated `IF NOT EXISTS` table, unique-index, and
pagination-index statements for the listed model owners. HNSW definitions
provide the index name, table, field path, dimension, and optional vector type,
distance, `efConstruction`, `m`, `concurrently`, and `defer` settings.
`applySchema(executor, definition)` runs those statements sequentially through
an executor with `query(statement)`. Raw statements must be made idempotent by
the caller; they are never rewritten.

Pass the same definition to a connection layer when schema must be applied
before the `Database` service is published:

```ts
const nodeLayer = makeNodeDatabaseLayer({
  endpoint: nodeEndpoint("mem"),
  namespace: "app",
  database: "app",
  schema: { models: [User] },
})
```

`models` is an explicit list. A table-backed view resolves to its real store
owner; a SQL view cannot define table or index DDL. Defining a model without
putting it in `schema.models` does not register global indexes. If connection
or schema startup fails, the layer closes the acquired client and publishes no
`Database` service.

Relation tables have a separate SurrealDB table type. The relation write path
lazily creates them as `TYPE RELATION SCHEMALESS`; keep that declaration
separate from ordinary model table DDL and do not register a relation table as
a normal model owner.

### SQL views

`Model.sqlView(name, rowSchema, options)` defines a read-only model with a
logical name and no table owner. `options.params` is the caller-input Schema,
`sql` is the static statement, an optional `bind` function is the binding
escape hatch, and `resultIndex` selects the statement result slot (default
`0`). `ViewStore.query(params)` encodes the caller input exactly once. Without
`bind`, the encoded value must be a plain object and its fields become named
bindings. With `bind`, the function receives that already-encoded value and a
`BoundQuery`; it may add native Surreal values and returns the query without
encoding the parameters again.

```ts
import { Schema } from "effect"
import { Table } from "surrealdb"
import { Model, RootIdSchema, makeView } from "@appdb/core"

const Row = Schema.Struct({ id: RootIdSchema, label: Schema.String })
const Params = Schema.Struct({ minScore: Schema.Int })
const Labels = Model.sqlView("labels", Row, {
  params: Params,
  sql: "RETURN NONE; SELECT record::id(id) AS id, label FROM user WHERE score >= $minScore;",
  resultIndex: 1,
})
const rows = makeView(Labels).query({ minScore: 10 })

const DynamicParams = Schema.Struct({ table: Schema.String, minScore: Schema.Int })
const DynamicLabels = Model.sqlView("dynamic_labels", Row, {
  params: DynamicParams,
  sql: "RETURN NONE; SELECT record::id(id) AS id, label FROM $table WHERE score >= $minScore;",
  bind: (statement, encoded) =>
    statement.append("", {
      table: new Table(encoded.table),
      minScore: encoded.minScore,
    }),
  resultIndex: 1,
})
const dynamicRows = makeView(DynamicLabels).query({ table: "user", minScore: 10 })
```

The first view uses the default plain-object binder. The second uses a custom
binder to turn the encoded table name into a Surreal `Table`; both select slot
`1` because slot `0` is the preceding `RETURN NONE` statement.

### Connections and services

- `Database` is the scoped connection service. Its `DatabaseApi` contains
  checked `query`, faithful `queryUnchecked`/`execute`, the underlying `client`,
  the `endpoint`, and an idempotent `close` Effect.
- `makeDatabaseLayer` and its explicit alias `makeRemoteDatabaseLayer` create a
  remote-only layer by default. `DatabaseLayerOptions.makeClient` is the
  injection seam for a caller-owned SDK client; `clientOptions` configures the
  default remote client.
- The Node entry point adds `makeNodeDatabaseLayer`, `NodeDatabaseLayer`,
  `nodeEndpoint`, and the `mem`, `rocksdb`, `surrealkv`, and
  `surrealkv+versioned` endpoint choices. Disk backends require an explicit
  path; `mem` does not accept one.

The connection layer registers the client with Effect `Scope` before it starts
the connection. Scope release closes the client, including when startup fails.
Close is idempotent, and startup failures are returned as `DbError` values.
An already-started SDK query is awaited in an uninterruptible boundary before
the Effect can finish and scope release can close the client. This preserves
the client lifetime across interruption, but a permanently pending SDK Promise
can delay both interruption and close.

### Queries and transactions

The query module owns one parameterized SQL representation:

- `rawSql`/`sql` and `appendSql` build SDK `BoundQuery` values.
- `buildModelQuery` validates tables and fields, and represents scalar values as
  bindings. `validateIdentifier`, `assertModelField`, `Predicate`, and
  `ModelQuerySpec` define that bounded query language.
- `query`, `queryChecked`, `queryUnchecked`, and `queryRaw` execute through the
  `Database` service. `queryTake`/`queryBoundTake` decode an array slot;
  `queryReturn`/`queryBoundReturn` decode an optional slot. The `queryBound*`
  names retain the Rust raw/bound naming split; `decodeSlot` decodes one slot.
- `TxStmt`, `TxResults`, `TxStatementResults`, `runTx`, and `tx` execute grouped
  transaction inputs and preserve one response group per input. The
  `TxInput`, `TxResponse`, and `TxError` types plus
  `isTransactionCapabilityError` describe the transaction boundary. An engine
  that cannot provide transactions fails with `TransactionCapabilityError`
  before execution when that capability is detectable.

`queryUnchecked` keeps the SDK per-statement response union. Checked query and
transaction helpers fail on response errors and preserve the statement index
in the resulting `DbError` operation. The package does not add an arbitrary
retry policy.

### Repositories, graph, relations, and pagination

- `makeStore` exposes `create`, `createAt`, `upsertAt`, `updateAt`, `save`,
  `saveMany`, `get`, `getRecord`, `list`, `exists`, `delete`, `deleteRecord`,
  `deleteAll`, `listRecordIds`, `findOneId`, `merge`, and `patch`.
- `makeStore(owner).returning(view)` exposes `create`, `createAt`, `upsertAt`,
  `updateAt`, and `save`, returning the typed value of an owner-preserving
  `Model.view(owner, schema)`. The view must belong to the written owner;
  SQL views and views from another owner fail with `InvalidModel`.
- Returning writes reuse the owner's single `WritePlan`. The plan's write
  statements return their normal `RETURN AFTER` rows into tracked result
  slots; the returning surface selects the parent slot and hydrates it with
  the view schema, using planned child slots for foreign fields. It does not
  issue a follow-up `SELECT` or rewrite the write SQL into a projection.
- `makeView` exposes `query(params)` for a SQL view model and `get`/`list` for
  a table-backed view model. `RepositoryError` is the union of `DbError` and
  `CryptoError` used by repository operations.
- `GraphRepo` and the free graph functions (`relateAt`, `backRelateAt`,
  `unrelateAt`, `unrelateAll`, `outgoingIds`/`outIds`,
  `incomingIds`/`inIds`, `outgoingRows`, `incomingRows`, counts, edges, and
  hydrated `outgoing`/`incoming` reads) cover the graph boundary. The relation
  module exposes `relateStatement`, `backRelateStatement`,
  `unrelateStatement`, `unrelateAllStatement`, the incoming/outgoing statement
  builders, `appendOrderedRelationEdges`, `assertRecordId`, and the edge types.
- `PageCursor`, `PaginationPlan`, `buildPaginationQuery`, `pageFromRows`,
  `queryPage`, `paginate`, `paginAsc`, and `paginDesc` provide immutable
  keyset pagination. `queryPage` returns the raw storage page; `paginate`
  builds the lookahead cursor from those raw rows before hydrating page items,
  so encoded pagination values remain the values used by the SQL comparison.
  Cursors carry table, field, order, value, and record-ID compatibility data.
- `hydrateRows` is the public batch decode/hydrate seam. It keeps one
  operation-local foreign/relation read context, batches candidate reads, and
  preserves input order; `hydrateRow` is the single-row form built on the same
  seam. Graph and view consumers use these public hydration boundaries.

### Errors and crypto

`DbError`, `DbErrorKind`, `makeDbError`, `isDbError`, `errorMessage`, and
`decodeError` are the database error boundary. `CryptoError` carries a stable
reason, optional key context, and an optional cause. Both error classes are
Effect tagged errors, so `Effect.catchTag` can handle them without matching
message text.

`Crypto` (also exported as `CryptoService`) exposes authenticated encryption
for bytes, text, and JSON-like values. `makeCryptoLayer`,
`makeStaticKeyProvider`, `makeStaticKeyLayer`, `CryptoLive`, and
`StaticKeyLayer` provide the provider and layer seams. AES-256-GCM uses a
32-byte key, a 12-byte nonce, and a 16-byte authentication tag. The exported
`CRYPTO_*` constants, `KeyContext`, `ResolvedKeyContext`, `CryptoApi`,
`KeyProvider`, `WebCryptoLike`, `CryptoLayerOptions`, `resolveKeyContext`,
`makeCryptoError`, `isCryptoError`, `isByteContainer`, and `normalizeBytes`
describe the provider and byte boundaries. Omitted service/account values
resolve to `appdb`/`master-sensitive`.

Sensitive schema metadata has two scopes. `scope: "value"` encrypts the whole
encoded field as one compact JSON value; `scope: "leaf"` walks arrays and
plain objects and encrypts their scalar leaves separately. `null` is preserved
and omitted/`undefined` values remain omitted at the field boundary. The
encrypted value path accepts
only the JSON-compatible values enforced by `encodeJson`: `bigint`,
`undefined`, functions, symbols, non-finite numbers, and unsafe integer values
are rejected. A schema whose encoded value needs a non-JSON representation
must use an explicit byte or text codec before it is marked sensitive; the
metadata does not make arbitrary Schema values losslessly JSON-encryptable.

The Node keyring provider (`makeNodeKeyringProvider` and
`makeNodeKeyringLayer`) dynamically loads `@napi-rs/keyring` when a key is
requested. Only `null` or `undefined` means that a keyring entry is missing and
may be generated. Empty, malformed, or incorrectly sized values are corruption
errors, and provider failures are propagated without generating a replacement
key. On Linux, a kernel-keyutils fallback is not durable across reboot; use a
durable Secret Service deployment when restart persistence is required. This
provider does not write a plaintext backup.

## Effect v4 guidance

The implementation targets the installed Effect `4.0.0-rc.115` API. The
official v4 material used for the service, layer, scope, schema, and expected
error boundaries is:

- [Getting started](https://effect.website/docs/v4/getting-started)
- [Services](https://effect.website/docs/v4/requirements-management/services)
- [Layers](https://effect.website/docs/v4/requirements-management/layers)
- [Scope and resource management](https://effect.website/docs/v4/resource-management/scope)
- [Schema](https://effect.website/docs/v4/schema/introduction)
- [Expected errors](https://effect.website/docs/v4/error-management/expected-errors)
- [Timing out Effects](https://effect.website/docs/v4/error-management/timing-out)
- [Data and tagged errors](https://effect.website/docs/v4/data-types/data)
- [Effect v4 API reference](https://effect.website/docs/v4/api)
- [Effect v4 migration source](https://github.com/Effect-TS/effect/blob/main/MIGRATION.md)
- [Effect source guidance](https://github.com/Effect-TS/effect/blob/main/LLMS.md)

These links describe the v4 API family. Code examples should be checked
against the pinned RC and its generated declarations; v3 `Context.Tag` or
`Effect.Service` recipes are not a compatibility contract for this package.

## SDK and native adapter patches

The workspace applies `patches/surrealdb@2.0.8.patch` through
`pnpm-workspace.yaml`. The patch changes both `surrealdb` ESM and CommonJS
builds in `ConnectionController.connect`:

1. subscribe to the current engine's `error` event before opening it, with an
   engine-identity guard so a late error from a replaced engine is ignored;
2. subscribe to the first `connected` or `error` event before `open`; and
3. convert a synchronous `open` throw into the same error path and rethrow the
   original `Error` (or an `UnexpectedConnectionError` for a non-Error value).

The upstream path waited only for `connected` and did not forward an engine
error emitted during startup. A native engine could therefore fail while the
SDK's `connect` Promise remained pending. The patch is limited to
`surrealdb@2.0.8` and does not change the embedded engine or the Rust
workspace.

The workspace also applies `patches/@surrealdb__node@3.0.3.patch` to the Node
native adapter. Its `NodeEngine.close` path captures the native `free()`
Promise, clears the adapter state and publishes `disconnected` in the upstream
order, then awaits that captured Promise. This makes `Surreal.close()` and the
Effect connection scope's release wait for native engine teardown while
preventing a reentrant reconnect from replacing the engine reference that is
being released. It does not change the Rust notification architecture or
native engine behavior.

Both patch files are included in the package tarball as reproducibility
artifacts. They are not automatically applied to a different consumer
dependency graph: a downstream workspace that chooses to use the package must
own and verify its own `patchedDependencies` configuration for both exact
package versions.

This package is marked `private`. The supported workspace path is to run
`pnpm install --frozen-lockfile`, `pnpm run build`, `pnpm run typecheck:example`,
and `pnpm run example` from this directory. No npm or tarball consumer path is
claimed here: a downstream
workspace that chooses to use the package must own and verify its own
`patchedDependencies` configuration. Merely depending on a packed tarball, or
receiving these patch files inside it, does not apply either patch to that
downstream install.

The package tarball is intentionally limited to `dist/`, this README, both
applicable patch files, and the required package metadata. It does not include
`node_modules`, tests, temporary database directories, or compiled native
artifacts.

## Runtime limitations

- The SDK query operation has no per-request `AbortSignal`. The connection and
  transaction owners await an already-started query or commit Promise through
  an uninterruptible boundary, so Scope does not close the client until the
  actual SDK Promise settles. `Effect.timeout` is a fiber boundary here and
  can itself be delayed until that Promise settles; a permanently pending
  Promise can block both the Effect and scope close.
- Effect timeout is a fiber boundary, not server-side SQL cancellation. It
  does not stop a request already sent through the SDK. Select an engine or
  server timeout when the deployment needs a bounded database operation.
- `mem://` is process-local and non-durable. Disk endpoints require a path and
  their durability, locking, and restart behavior come from the selected
  native engine. Remote endpoints require a reachable SurrealDB server.
- A Windows cross-process RocksDB handoff has been verified for an explicit
  path: one Node process wrote and closed the database, and a second process
  reopened that path and read the persisted value. This covers the exercised
  sequential handoff, not concurrent-writer behavior.
- The current Windows native-package probe fails while opening `surrealkv://`
  with `GenericFailure` / OS error `5` (the versioned form is covered by the
  same unsupported boundary). SurrealKV on Windows is therefore not a working
  backend for this package; this result does not describe other operating
  systems or imply a general SurrealKV limitation.
- Transaction support is a capability of the selected engine. Handle
  `TransactionCapabilityError` when a transaction is unavailable.
- The TypeScript SDK and native adapter versions above are separate from the
  Rust workspace version. Upgrade or replace them only with a new compatibility
  check and runtime probe.

## Development checks

From this directory:

```bash
pnpm install --frozen-lockfile
pnpm run typecheck
pnpm run build
pnpm run typecheck:example
pnpm run example
pnpm test
pnpm pack --dry-run
```

`typecheck:example` resolves the built package exports, so it checks the same
`@appdb/core` and `@appdb/core/node` boundary used by the runnable example.
