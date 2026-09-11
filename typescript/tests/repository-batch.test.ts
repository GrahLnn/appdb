import { Effect, Schema } from "effect"
import { describe, expect, it } from "vitest"

import { Database, type DatabaseApi } from "../src/connection.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeStore, hydrateRows } from "../src/repository.js"
import { field } from "../src/schema.js"

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "repository-batch",
    database: "repository-batch",
  }))))

const countQueries = (database: DatabaseApi): {
  readonly count: () => number
  readonly statements: readonly string[]
} => {
  let count = 0
  const statements: string[] = []
  const original = database.query
  const instrumented = database as unknown as { query: typeof database.query }
  instrumented.query = ((statement: Parameters<typeof original>[0], bindings?: Parameters<typeof original>[1]) => {
    count += 1
    statements.push(typeof statement === "string" ? statement : statement.query)
    return original.call(database, statement, bindings)
  }) as typeof original
  return { count: () => count, statements }
}

describe("repository batch hydration", () => {
  it("batches relation edges and target rows while preserving duplicates", async () => {
    const Target = Model.define("repository_batch_target", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    }))
    const Parent = Model.define("repository_batch_parent", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      targets: field(Schema.Array(Target.schema), {
        relate: { target: () => Target, direction: "outgoing", relation: "repository_batch_edges" },
      }),
    }))
    const store = makeStore(Parent)
    const values = Array.from({ length: 8 }, (_, index) => ({
      id: `parent-${index}`,
      targets: [
        { id: `target-${index}-a`, label: "a" },
        { id: `target-${index}-b`, label: "b" },
        { id: `target-${index}-a`, label: "a" },
      ],
    }))

    const result = await withMemDatabase(Effect.gen(function* () {
      const database = yield* Database
      const queries = countQueries(database)
      const saved = yield* store.saveMany(values)
      const afterSave = queries.count()
      const listed = yield* store.list()
      const afterList = queries.count()
      return { saved, listed, afterSave, afterList, statements: queries.statements }
    }))

    expect(result.afterSave).toBe(2)
    expect(result.afterList - result.afterSave).toBe(3)
    expect((result.statements[0]!.match(/DEFINE TABLE IF NOT EXISTS repository_batch_edges TYPE RELATION SCHEMALESS/g) ?? []).length).toBe(1)
    expect(result.saved).toEqual(values)
    expect([...result.listed].sort((left, right) => String(left.id).localeCompare(String(right.id)))).toEqual(
      [...values].sort((left, right) => left.id.localeCompare(right.id)),
    )
  })

  it("freezes a target thunk for one action and resolves it again on the next", async () => {
    const First = Model.define("repository_batch_first", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    }))
    const Second = Model.define("repository_batch_second", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    }))
    let active: typeof First | typeof Second = First
    let targetCalls = 0
    const Parent = Model.define("repository_batch_switch_parent", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      targets: field(Schema.Array(First.schema), {
        relate: {
          target: () => {
            targetCalls += 1
            return active as typeof First
          },
          direction: "outgoing",
          relation: "repository_batch_switch_edges",
        },
      }),
    }))
    const store = makeStore(Parent)

    const result = await withMemDatabase(Effect.gen(function* () {
      const database = yield* Database
      yield* store.save({ id: "parent", targets: [{ id: "first", label: "first" }] })
      const callsAfterFirstAction = targetCalls
      yield* database.query(
        "DELETE FROM repository_batch_switch_edges WHERE in = repository_batch_switch_parent:parent; CREATE repository_batch_second:second CONTENT { label: 'second' }; RELATE repository_batch_switch_parent:parent -> repository_batch_switch_edges -> repository_batch_second:second SET position = 0;",
      )
      active = Second
      const listed = yield* store.list()
      return { callsAfterFirstAction, totalCalls: targetCalls, listed }
    }))

    expect(result.callsAfterFirstAction).toBe(1)
    expect(result.totalCalls).toBe(2)
    expect(result.listed).toEqual([{ id: "parent", targets: [{ id: "second", label: "second" }] }])
  })

  it("exposes the same operation-local context through hydrateRows", async () => {
    const Target = Model.define("repository_batch_public_target", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    }))
    const Parent = Model.define("repository_batch_public_parent", Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      target: field(Target.schema, {
        relate: { target: () => Target, direction: "outgoing", relation: "repository_batch_public_edges" },
      }),
    }))
    const store = makeStore(Parent)

    const result = await withMemDatabase(Effect.gen(function* () {
      const database = yield* Database
      yield* store.save({ id: "parent", target: { id: "target", label: "value" } })
      const rows = yield* database.query("SELECT *, record::id(id) AS id FROM repository_batch_public_parent;")
      const queries = countQueries(database)
      const hydrated = yield* hydrateRows(Parent, rows[0] as readonly unknown[])
      return { hydrated, queryDelta: queries.count() }
    }))

    expect(result.queryDelta).toBe(2)
    expect(result.hydrated).toEqual([{ id: "parent", target: { id: "target", label: "value" } }])
  })
})
