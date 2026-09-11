import { Effect, Schema } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { GraphRepo } from "../src/graph.js"
import { RootIdSchema, makeRecordId } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { field } from "../src/schema.js"

const Person = Model.define(
  "graph_person",
  Schema.Struct({
    id: field(RootIdSchema, { id: true }),
    name: Schema.String,
  }),
)

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({ endpoint: "mem://", namespace: "graph", database: "graph" }),
      ),
    ),
  )

describe("Effect graph operations", () => {
  it("keeps relation result slots and hydrates typed rows through the repository seam", async () => {
    const alice = makeRecordId("graph_person", "alice")
    const bob = makeRecordId("graph_person", "bob")
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("CREATE graph_person:alice CONTENT { name: 'Alice' }; CREATE graph_person:bob CONTENT { name: 'Bob' };")
        yield* GraphRepo.relateAt(alice, bob, "graph_likes")
        const ids = yield* GraphRepo.outgoingIds(alice, "graph_likes")
        const rows = yield* GraphRepo.outgoingRows(alice, "graph_likes", Person.table)
        const typed = yield* GraphRepo.outgoing(alice, "graph_likes", Person)
        const incoming = yield* GraphRepo.incoming(bob, "graph_likes", Person)
        const count = yield* GraphRepo.outgoingCountAs(alice, "graph_likes", Person)
        return { ids, rows, typed, incoming, count }
      }),
    )

    expect(result.ids).toEqual([bob])
    expect(result.rows).toEqual([{ id: "bob", name: "Bob" }])
    expect(result.typed).toEqual([{ id: "bob", name: "Bob" }])
    expect(result.incoming).toEqual([{ id: "alice", name: "Alice" }])
    expect(result.count).toBe(1)
  })
})

