import { Context, Effect, Layer, Schema, SchemaGetter } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { Model } from "../src/model.js"
import { makeStore, makeView } from "../src/repository.js"
import { field } from "../src/schema.js"
import { RootIdSchema } from "../src/id.js"

interface CodecState {
  readonly prefix: string
  encodeCalls: number
  decodeCalls: number
}

class CodecEnv extends Context.Service<CodecEnv, CodecState>()("appdb/tests/CodecEnv") {}

const makeMemLayer = () =>
  makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "composition",
    database: "composition",
  })

const withServices = <A, E>(
  effect: Effect.Effect<A, E, Database | CodecEnv>,
  state: CodecState,
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        Effect.provide(effect, makeMemLayer()),
        Layer.succeed(CodecEnv, state),
      ),
    ),
  )

const withDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeMemLayer())))

const prefixedNumber = Schema.String.pipe(
  Schema.decodeTo(Schema.Number, {
    decode: SchemaGetter.transformEffect<number, string, CodecEnv>((encoded) =>
      CodecEnv.use((env) => {
        env.decodeCalls += 1
        return Effect.succeed(Number(encoded.slice(env.prefix.length)))
      }),
    ),
    encode: SchemaGetter.transformEffect<string, number, CodecEnv>((decoded) =>
      CodecEnv.use((env) => {
        env.encodeCalls += 1
        return Effect.succeed(`${env.prefix}${decoded}`)
      }),
    ),
  }),
)

const ChildSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  value: prefixedNumber,
})
const Child = Model.define("composed_child", ChildSchema)

const ParentSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  child: field(ChildSchema, { foreign: { target: () => Child } }),
  maybe: Schema.optionalKey(Schema.String),
  nullable: Schema.NullOr(Schema.String),
})
const Parent = Model.define("composed_parent", ParentSchema)

describe("consumer composition boundaries", () => {
  it("saves and gets a foreign non-identity codec exactly once per public action", async () => {
    const state: CodecState = { prefix: "n:", encodeCalls: 0, decodeCalls: 0 }
    const store = makeStore(Parent)
    const input = {
      id: "parent-1",
      child: { id: "child-1", value: 7 },
      nullable: null,
    }

    const result = await withServices(
      Effect.gen(function* () {
        const beforeSave = { encode: state.encodeCalls, decode: state.decodeCalls }
        const saved = yield* store.save(input)
        const saveCalls = {
          encode: state.encodeCalls - beforeSave.encode,
          decode: state.decodeCalls - beforeSave.decode,
        }

        const beforeGet = { encode: state.encodeCalls, decode: state.decodeCalls }
        const fetched = yield* store.get("parent-1")
        const getCalls = {
          encode: state.encodeCalls - beforeGet.encode,
          decode: state.decodeCalls - beforeGet.decode,
        }
        return { saved, fetched, saveCalls, getCalls }
      }),
      state,
    )

    expect(result.saveCalls).toEqual({ encode: 1, decode: 1 })
    expect(result.getCalls).toEqual({ encode: 0, decode: 1 })
    expect(result.saved).toEqual({
      id: "parent-1",
      child: { id: "child-1", value: 7 },
      nullable: null,
    })
    expect(result.fetched).toEqual(result.saved)
  })

  it("keeps two projections on one table tied to their own view schema identity", async () => {
    const Owner = Model.define(
      "same_table_owner",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
        score: Schema.Int,
        hidden: Schema.String,
      }),
    )
    const LabelView = Model.view(
      Owner,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const ScoreView = Model.view(
      Owner,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        score: Schema.Int,
      }),
    )
    const labels = makeView(LabelView)
    const scores = makeView(ScoreView)

    expect(LabelView.source.kind).toBe("view")
    expect(ScoreView.source.kind).toBe("view")
    expect(LabelView.source.identity).not.toBe(ScoreView.source.identity)
    expect(LabelView.source.ownerIdentity).toBe(Owner.source.ownerIdentity)
    expect(ScoreView.source.ownerIdentity).toBe(Owner.source.ownerIdentity)

    const result = await withDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE same_table_owner:one CONTENT { label: 'visible', score: 42, hidden: 'private' };",
        )
        const label = yield* labels.get("one")
        const score = yield* scores.get("one")
        return { label, score }
      }),
    )

    expect(result.label).toEqual({ id: "one", label: "visible" })
    expect(result.score).toEqual({ id: "one", score: 42 })
  })

  it("distinguishes a missing optional key from null and preserves optionalKey rejection of undefined", async () => {
    const Optional = Model.define(
      "none_null_consumer",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        maybe: Schema.optionalKey(Schema.String),
        nullable: Schema.NullOr(Schema.String),
      }),
    )
    const store = makeStore(Optional)

    expect(() => Schema.decodeUnknownSync(Optional.schema)({
      id: "explicit-undefined",
      maybe: undefined,
      nullable: null,
    })).toThrow()

    const result = await withDatabase(
      Effect.gen(function* () {
        const saved = yield* store.save({ id: "missing", nullable: null })
        const fetched = yield* store.get("missing")
        return { saved, fetched }
      }),
    )

    expect(Object.hasOwn(result.saved, "maybe")).toBe(false)
    expect(Object.hasOwn(result.fetched, "maybe")).toBe(false)
    expect(result.saved.nullable).toBeNull()
    expect(result.fetched.nullable).toBeNull()
  })
})
