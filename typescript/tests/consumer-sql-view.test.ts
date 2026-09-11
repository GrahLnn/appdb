import { Context, Effect, Exit, Layer, Option, Schema, SchemaGetter } from "effect"
import { BoundQuery, Table } from "surrealdb"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { DbErrorKind } from "../src/errors.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeView } from "../src/repository.js"
import { field } from "../src/schema.js"

interface ViewCodecState {
  paramEncodeCalls: number
  rowDecodeCalls: number
}

class ViewCodecEnv extends Context.Service<ViewCodecEnv, ViewCodecState>()(
  "appdb/tests/ViewCodecEnv",
) {}

const withDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "consumer-sql-view",
          database: "consumer-sql-view",
        }),
      ),
    ),
  )

const withCodecServices = <A, E>(
  effect: Effect.Effect<A, E, Database | ViewCodecEnv>,
  state: ViewCodecState,
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        Effect.provide(
          effect,
          makeNodeDatabaseLayer({
            endpoint: "mem://",
            namespace: "consumer-sql-view",
            database: "consumer-sql-view",
          }),
        ),
        Layer.succeed(ViewCodecEnv, state),
      ),
    ),
  )

const encodedParamText = Schema.String.pipe(
  Schema.decodeTo(Schema.String, {
    decode: SchemaGetter.transformEffect<string, string, ViewCodecEnv>((value) =>
      Effect.succeed(value),
    ),
    encode: SchemaGetter.transformEffect<string, string, ViewCodecEnv>((value) =>
      ViewCodecEnv.use((state) => {
        state.paramEncodeCalls += 1
        return Effect.succeed(`param:${value}`)
      }),
    ),
  }),
)

const encodedRowLabel = Schema.String.pipe(
  Schema.decodeTo(Schema.String, {
    decode: SchemaGetter.transformEffect<string, string, ViewCodecEnv>((value) =>
      ViewCodecEnv.use((state) => {
        state.rowDecodeCalls += 1
        return Effect.succeed(value.replace(/^row:/, ""))
      }),
    ),
    encode: SchemaGetter.transformEffect<string, string, ViewCodecEnv>((value) =>
      Effect.succeed(value),
    ),
  }),
)

describe("consumer SQL view queries", () => {
  it("uses the default parameter codec once, hydrates each SQL row once, and rejects get/list", async () => {
    const Params = Schema.Struct({ term: encodedParamText })
    const Rows = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: encodedRowLabel,
    })
    const SqlView = Model.sqlView("consumer_sql_default_logical", Rows, {
      params: Params,
      sql: "RETURN NONE; SELECT record::id(id) AS id, label FROM consumer_sql_default_source WHERE lookup = $term ORDER BY id ASC;",
      resultIndex: 1,
    })
    const view = makeView(SqlView)
    const state: ViewCodecState = { paramEncodeCalls: 0, rowDecodeCalls: 0 }

    const result = await withCodecServices(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE consumer_sql_default_source:one CONTENT { lookup: 'param:visible', label: 'row:visible' };",
        )
        const rows = yield* view.query({ term: "visible" })
        const get = yield* Effect.exit(view.get("one"))
        const list = yield* Effect.exit(view.list())
        return { rows, get, list }
      }),
      state,
    )

    expect(result.rows).toEqual([{ id: "one", label: "visible" }])
    expect(state.paramEncodeCalls).toBe(1)
    expect(state.rowDecodeCalls).toBe(1)
    expect(Exit.isFailure(result.get)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.get))).toMatchObject({
      kind: DbErrorKind.InvalidModel,
      operation: "view.get",
    })
    expect(Exit.isFailure(result.list)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.list))).toMatchObject({
      kind: DbErrorKind.InvalidModel,
      operation: "view.list",
    })
  })

  it("lets a custom binder supply a Surreal Table and uses the declared result slot", async () => {
    const Params = Schema.Struct({
      table: Schema.String,
      lookup: Schema.String,
    })
    const Rows = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    })
    let bindCalls = 0
    const SqlView = Model.sqlView("consumer_sql_custom_logical", Rows, {
      params: Params,
      sql: "RETURN NONE; SELECT record::id(id) AS id, label FROM $table WHERE lookup = $lookup ORDER BY id ASC;",
      bind: (statement, params) => {
        bindCalls += 1
        return new BoundQuery(statement.query, {
          ...statement.bindings,
          table: new Table(params.table),
          lookup: params.lookup,
        })
      },
      resultIndex: 1,
    })
    const view = makeView(SqlView)

    const rows = await withDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE consumer_sql_custom_source:one CONTENT { lookup: 'visible', label: 'bound' };",
        )
        return yield* view.query({ table: "consumer_sql_custom_source", lookup: "visible" })
      }),
    )

    expect(rows).toEqual([{ id: "one", label: "bound" }])
    expect(bindCalls).toBe(1)
  })
})
