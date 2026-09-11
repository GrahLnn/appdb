import { Context, Effect, Layer } from "effect"
import {
  BoundQuery,
  createRemoteEngines,
  Surreal,
  type ConnectOptions,
  type DriverOptions,
  type QueryResponse,
} from "surrealdb"
import { dbErrorFromCause, DbError } from "./errors.js"
import type { DbError as DbErrorType } from "./errors.js"
import { checkedResponseSlots } from "./query.js"
import { applySchema, type SchemaDdlDefinition } from "./schema-ddl.js"

export type DatabaseQuery = string | BoundQuery
export type DatabaseBindings = Readonly<Record<string, unknown>>
export type DatabaseResponse = QueryResponse<unknown>

type DatabaseErrorKind = DbError["kind"]
type ConnectionErrorKind = Extract<DatabaseErrorKind, "Transport" | "Engine">

export interface DatabaseApi {
  /**
   * Execute all statements and fail the Effect when any statement fails.
   * The successful values retain one slot per statement, including
   * `undefined` for successful BEGIN and COMMIT statements.
   */
  readonly query: (
    query: DatabaseQuery,
    bindings?: DatabaseBindings,
  ) => Effect.Effect<ReadonlyArray<unknown>, DbError>
  /**
   * Execute all statements while retaining the SDK's per-statement response
   * union. This is the faithful equivalent of the SDK `.responses()` path.
   */
  readonly queryUnchecked: (
    query: DatabaseQuery,
    bindings?: DatabaseBindings,
  ) => Effect.Effect<ReadonlyArray<DatabaseResponse>, DbError>
  /** Alias for `queryUnchecked`; both paths share one request implementation. */
  readonly execute: (
    query: DatabaseQuery,
    bindings?: DatabaseBindings,
  ) => Effect.Effect<ReadonlyArray<DatabaseResponse>, DbError>
  /** The underlying SDK handle for advanced operations. Layer scope owns it. */
  readonly client: Surreal
  readonly endpoint: string | URL
  /** Close is idempotent; the owning Layer also runs it at scope finalization. */
  readonly close: Effect.Effect<void, DbError>
}

/** Effect service for one scoped SurrealDB connection. */
export class Database extends Context.Service<Database, DatabaseApi>()("appdb/connection/Database") {}

export interface DatabaseLayerOptions {
  readonly endpoint: string | URL
  readonly namespace?: string
  readonly database?: string
  /** Optional raw/model schema applied after connect and before service publication. */
  readonly schema?: SchemaDdlDefinition
  /**
   * Construct the client that owns the engine map. The default uses only the
   * SDK's remote engines; the Node entry point supplies native engines here.
   */
  readonly makeClient?: () => Surreal
  /** Driver options used only when `makeClient` is omitted. */
  readonly clientOptions?: DriverOptions
  /** Error family for connection and engine startup failures. */
  readonly connectionErrorKind?: ConnectionErrorKind
}

interface ClientState {
  closed: boolean
}

const wrapDbError = dbErrorFromCause

const withQueryOperation = (error: DbErrorType): DbErrorType =>
  error.operation?.startsWith("query result ")
    ? new DbError({
        kind: error.kind,
        message: error.message,
        operation: "query",
        ...(error.cause === undefined ? {} : { cause: error.cause }),
      })
    : error

const makeConnectOptions = (options: DatabaseLayerOptions): ConnectOptions => ({
  ...(options.namespace === undefined ? {} : { namespace: options.namespace }),
  ...(options.database === undefined ? {} : { database: options.database }),
})

const makeDefaultClient = (options: DriverOptions | undefined): Surreal =>
  new Surreal({
    ...(options ?? {}),
    engines: options?.engines ?? createRemoteEngines(),
  })

const copyBindings = (bindings: DatabaseBindings): Record<string, unknown> => ({ ...bindings })

const makeBoundQuery = (query: BoundQuery, bindings: DatabaseBindings | undefined): BoundQuery => {
  if (bindings === undefined) return query
  return new BoundQuery(query).append("", copyBindings(bindings))
}

const runQuery = (
  client: Surreal,
  query: DatabaseQuery,
  bindings: DatabaseBindings | undefined,
) => {
  if (typeof query === "string") {
    return bindings === undefined
      ? client.query<unknown[]>(query)
      : client.query<unknown[]>(query, copyBindings(bindings))
  }
  return client.query<unknown[]>(makeBoundQuery(query, bindings))
}

const makeCloseEffect = (
  client: Surreal,
  state: ClientState,
): Effect.Effect<void, DbError> =>
  Effect.suspend(() => {
    if (state.closed) return Effect.succeed(undefined)
    return Effect.tryPromise({
      try: async () => {
        await client.close()
        state.closed = true
      },
      catch: (cause) => wrapDbError("close", cause, "Engine"),
    })
  })

const releaseClient = (client: Surreal, state: ClientState): Effect.Effect<void, never> =>
  makeCloseEffect(client, state).pipe(Effect.catchCause((cause) => Effect.die(cause)))

const makeDatabaseApi = (
  client: Surreal,
  endpoint: string | URL,
  state: ClientState,
): DatabaseApi => {
  const queryResponses = (
    input: DatabaseQuery,
    bindings: DatabaseBindings | undefined,
    operation: string,
  ): Effect.Effect<ReadonlyArray<DatabaseResponse>, DbError> =>
    // The SDK has no per-request AbortSignal. Keep the native promise
    // interruptible only after it settles, so Scope cannot free an embedded
    // engine while its N-API execute future still owns callbacks.
    Effect.uninterruptible(
      Effect.tryPromise({
        try: async () =>
          (await runQuery(client, input, bindings).responses()) as ReadonlyArray<DatabaseResponse>,
        catch: (cause) => wrapDbError(operation, cause, "Engine"),
      }),
    )

  const query = (
    input: DatabaseQuery,
    bindings?: DatabaseBindings,
  ): Effect.Effect<ReadonlyArray<unknown>, DbError> =>
    queryResponses(input, bindings, "query").pipe(
      Effect.flatMap((responses) => checkedResponseSlots(responses, "query")),
      Effect.mapError(withQueryOperation),
    )

  const queryUnchecked = (
    input: DatabaseQuery,
    bindings?: DatabaseBindings,
  ): Effect.Effect<ReadonlyArray<DatabaseResponse>, DbError> =>
    queryResponses(input, bindings, "queryUnchecked")

  const close = makeCloseEffect(client, state)

  return {
    query,
    queryUnchecked,
    execute: queryUnchecked,
    client,
    endpoint,
    close,
  }
}

const acquireClient = (
  options: DatabaseLayerOptions,
): Effect.Effect<{ client: Surreal; state: ClientState }, DbError> =>
  Effect.try({
    try: () => ({
      client: (options.makeClient ?? (() => makeDefaultClient(options.clientOptions)))(),
      state: { closed: false },
    }),
    catch: (cause) => wrapDbError("createClient", cause, options.connectionErrorKind ?? "Transport"),
  })

/**
 * Build a scoped connection layer around a caller-owned client factory.
 * The client is registered with Scope before connect/use can fail, so every
 * startup path has one close owner and no startup timeout is needed.
 */
export const makeDatabaseLayer = (
  options: DatabaseLayerOptions,
): Layer.Layer<Database, DbError, never> => {
  const resource = Effect.acquireRelease(
    acquireClient(options),
    ({ client, state }) => releaseClient(client, state),
  )

  const connected = resource.pipe(
    Effect.flatMap(({ client, state }) =>
      Effect.gen(function* () {
        const api = yield* Effect.tryPromise({
          try: async () => {
            await client.connect(options.endpoint, makeConnectOptions(options))
            return makeDatabaseApi(client, options.endpoint, state)
          },
          catch: (cause) =>
            wrapDbError("connect", cause, options.connectionErrorKind ?? "Transport"),
        })
        if (options.schema !== undefined) {
          yield* applySchema(api, options.schema)
        }
        return api
      }),
    ),
  )

  return Layer.effect(Database, connected)
}

/** Explicit alias for callers that want the remote-only default layer. */
export const makeRemoteDatabaseLayer = makeDatabaseLayer
