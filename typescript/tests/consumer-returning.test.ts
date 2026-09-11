import { Context, Effect, Exit, Layer, Option, Schema, SchemaGetter } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { DbErrorKind } from "../src/errors.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeStore } from "../src/repository.js"
import { field, foreign, relation } from "../src/schema.js"

interface ReturningCodecState {
  inputEncodeCalls: number
  viewDecodeCalls: number
}

class ReturningInputEnv extends Context.Service<ReturningInputEnv, ReturningCodecState>()(
  "appdb/tests/ReturningInputEnv",
) {}

class ReturningViewEnv extends Context.Service<ReturningViewEnv, ReturningCodecState>()(
  "appdb/tests/ReturningViewEnv",
) {}

const withServices = <A, E>(
  effect: Effect.Effect<A, E, Database | ReturningInputEnv | ReturningViewEnv>,
  inputState: ReturningCodecState,
  viewState: ReturningCodecState,
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        Effect.provide(
          Effect.provide(
            effect,
            makeNodeDatabaseLayer({
              endpoint: "mem://",
              namespace: "consumer-returning",
              database: "consumer-returning",
            }),
          ),
          Layer.succeed(ReturningInputEnv, inputState),
        ),
        Layer.succeed(ReturningViewEnv, viewState),
      ),
    ),
  )

const withDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "consumer-returning",
          database: "consumer-returning",
        }),
      ),
    ),
  )

const encodedInputLabel = Schema.String.pipe(
  Schema.decodeTo(Schema.String, {
    decode: SchemaGetter.transformEffect<string, string, ReturningInputEnv>((value) =>
      Effect.succeed(value.replace(/^stored:/, "")),
    ),
    encode: SchemaGetter.transformEffect<string, string, ReturningInputEnv>((value) =>
      ReturningInputEnv.use((state) => {
        state.inputEncodeCalls += 1
        return Effect.succeed(`stored:${value}`)
      }),
    ),
  }),
)

const returningViewLabel = Schema.String.pipe(
  Schema.decodeTo(Schema.String, {
    decode: SchemaGetter.transformEffect<string, string, ReturningViewEnv>((value) =>
      ReturningViewEnv.use((state) => {
        state.viewDecodeCalls += 1
        return Effect.succeed(value.replace(/^stored:/, ""))
      }),
    ),
    encode: SchemaGetter.transformEffect<string, string, ReturningViewEnv>((value) =>
      Effect.succeed(value),
    ),
  }),
)

describe("consumer write returning views", () => {
  it("returns the same write's owner View after foreign and relation writes", async () => {
    const Child = Model.define(
      "consumer_returning_child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Target = Model.define(
      "consumer_returning_target",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Parent = Model.define(
      "consumer_returning_parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: encodedInputLabel,
        child: foreign(() => Child),
        targets: relation(() => Target, {
          direction: "outgoing",
          relation: "consumer_returning_edges",
          cardinality: "many",
        }),
      }),
    )
    const ParentView = Model.view(
      Parent,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: returningViewLabel,
      }),
    )
    const store = makeStore(Parent)
    const returning = store.returning(ParentView)
    const inputState: ReturningCodecState = { inputEncodeCalls: 0, viewDecodeCalls: 0 }
    const viewState: ReturningCodecState = { inputEncodeCalls: 0, viewDecodeCalls: 0 }

    const result = await withServices(
      Effect.gen(function* () {
        const database = yield* Database
        const returned = yield* returning.createAt("parent", {
          id: "parent",
          label: "owner",
          child: { id: "child", label: "nested" },
          targets: [
            { id: "target-a", label: "A" },
            { id: "target-b", label: "B" },
          ],
        })
        const children = yield* database.query(
          "SELECT record::id(id) AS id FROM consumer_returning_child ORDER BY id ASC;",
        )
        const targets = yield* database.query(
          "SELECT record::id(id) AS id FROM consumer_returning_target ORDER BY id ASC;",
        )
        const edges = yield* database.query(
          "SELECT record::id(`in`) AS source, record::id(out) AS target, position FROM consumer_returning_edges ORDER BY position ASC;",
        )
        return { returned, children, targets, edges }
      }),
      inputState,
      viewState,
    )

    expect(result.returned).toEqual({ id: "parent", label: "owner" })
    expect(inputState.inputEncodeCalls).toBe(1)
    expect(viewState.viewDecodeCalls).toBe(1)
    expect(result.children[0]).toEqual([{ id: "child" }])
    expect(result.targets[0]).toEqual([{ id: "target-a" }, { id: "target-b" }])
    expect(result.edges[0]).toEqual([
      { source: "parent", target: "target-a", position: 0 },
      { source: "parent", target: "target-b", position: 1 },
    ])
  })

  it("rejects cross-owner and SQL Views before any write is planned", async () => {
    const Source = Model.define(
      "consumer_returning_reject_source",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Other = Model.define(
      "consumer_returning_reject_other",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const OtherView = Model.view(
      Other,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const SqlView = Model.sqlView(
      "consumer_returning_reject_sql",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
      {
        params: Schema.Struct({}),
        sql: "RETURN [];",
      },
    )
    const sourceStore = makeStore(Source)
    const otherStore = makeStore(Other)

    const result = await withDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "DEFINE TABLE IF NOT EXISTS consumer_returning_reject_source SCHEMALESS; DEFINE TABLE IF NOT EXISTS consumer_returning_reject_other SCHEMALESS;",
        )
        const input = { id: "rejected", label: "never-written" }
        const crossOwner = yield* Effect.exit(
          sourceStore.returning(OtherView).createAt("rejected", input),
        )
        const sqlView = yield* Effect.exit(
          sourceStore.returning(SqlView).createAt("rejected-sql", {
            id: "rejected-sql",
            label: "never-written",
          }),
        )
        const sourceExists = yield* sourceStore.exists()
        const otherExists = yield* otherStore.exists()
        return { crossOwner, sqlView, sourceExists, otherExists }
      }),
    )

    expect(Exit.isFailure(result.crossOwner)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.crossOwner))).toMatchObject({
      kind: DbErrorKind.InvalidModel,
      operation: "write.returning",
    })
    expect(Exit.isFailure(result.sqlView)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.sqlView))).toMatchObject({
      kind: DbErrorKind.InvalidModel,
      operation: "write.returning",
    })
    expect(result.sourceExists).toBe(false)
    expect(result.otherExists).toBe(false)
  })
})
