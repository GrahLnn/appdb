import { Effect, Exit, Schema } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeStore } from "../src/repository.js"
import { field } from "../src/schema.js"

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "repository-returning-fallback",
          database: "repository-returning-fallback",
        }),
      ),
    ),
  )

describe("repository returning and fallback lookup", () => {
  it("returns an owner-preserving projection from the same write plan", async () => {
    const Child = Model.define(
      "returning_child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Owner = Model.define(
      "returning_owner",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Child.schema, { foreign: { target: () => Child } }),
        hidden: Schema.String,
      }),
    )
    const Projection = Model.view(
      Owner,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Child.schema, { foreign: { target: () => Child } }),
      }),
    )

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const returned = makeStore(Owner).returning(Projection)
        return yield* returned.createAt("owner-1", {
          id: "owner-1",
          child: { id: "child-1", label: "nested" },
          hidden: "private",
        })
      }),
    )

    expect(result).toEqual({ id: "owner-1", child: { id: "child-1", label: "nested" } })
  })

  it("rejects SQL or cross-owner projections at the returning boundary", async () => {
    const Owner = Model.define(
      "returning_boundary_owner",
      Schema.Struct({ id: field(RootIdSchema, { id: true }), label: Schema.String }),
    )
    const Other = Model.define(
      "returning_boundary_other",
      Schema.Struct({ id: field(RootIdSchema, { id: true }), label: Schema.String }),
    )
    const OtherView = Model.view(Other, Other.schema)
    const SqlView = Model.sqlView("returning_boundary_sql", Owner.schema, {
      params: Schema.Void,
      sql: "SELECT id, label FROM returning_boundary_owner;",
    })

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const store = makeStore(Owner)
        const crossOwner = yield* Effect.exit(
          store.returning(OtherView).createAt("other-1", { id: "other-1", label: "wrong" }),
        )
        const sql = yield* Effect.exit(
          store.returning(SqlView).createAt("owner-1", { id: "owner-1", label: "wrong" }),
        )
        return { crossOwner, sql }
      }),
    )

    expect(result.crossOwner).toMatchObject({ _tag: "Failure" })
    expect(result.sql).toMatchObject({ _tag: "Failure" })
    expect(Exit.findErrorOption(result.crossOwner)).toMatchObject({ _tag: "Some", value: { kind: "InvalidModel" } })
    expect(Exit.findErrorOption(result.sql)).toMatchObject({ _tag: "Some", value: { kind: "InvalidModel" } })
  })

  it("uses all safe fields as the fallback identity when no unique field exists", async () => {
    const Fallback = Model.define(
      "fallback_identity",
      Schema.Struct({ label: Schema.String, rank: Schema.Int }),
    )
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const store = makeStore(Fallback)
        const db = yield* Database
        yield* db.query("DEFINE TABLE fallback_identity SCHEMALESS;")
        const first = yield* store.save({ label: "same", rank: 1 })
        const second = yield* store.save({ label: "same", rank: 1 })
        const rows = yield* db.query("SELECT * FROM fallback_identity;")
        void [first, second]
        return rows[0]
      }),
    )

    expect(result).toHaveLength(1)
  })

  it("keeps id-less fallback payloads distinct within saveMany", async () => {
    const Fallback = Model.define(
      "fallback_batch_identity",
      Schema.Struct({ label: Schema.String, rank: Schema.Int }),
    )
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const store = makeStore(Fallback)
        const db = yield* Database
        yield* db.query("DEFINE TABLE fallback_batch_identity SCHEMALESS;")
        yield* store.save({ label: "same", rank: 1 })
        yield* store.saveMany([
          { label: "same", rank: 1 },
          { label: "same", rank: 1 },
        ])
        const rows = yield* db.query("SELECT * FROM fallback_batch_identity;")
        return rows[0]
      }),
    )

    expect(result).toHaveLength(3)
  })

  it("resolves foreign fallback identities before choosing a new child", async () => {
    const Child = Model.define(
      "fallback_child",
      Schema.Struct({ label: Schema.String }),
    )
    const Parent = Model.define(
      "fallback_parent",
      Schema.Struct({ child: field(Child.schema, { foreign: { target: () => Child } }) }),
    )

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const store = makeStore(Parent)
        const db = yield* Database
        yield* db.query("DEFINE TABLE fallback_child SCHEMALESS; DEFINE TABLE fallback_parent SCHEMALESS;")
        const value = { child: { label: "same-child" } }
        yield* store.save(value)
        yield* store.save(value)
        const children = yield* db.query("SELECT * FROM fallback_child;")
        const parents = yield* db.query("SELECT * FROM fallback_parent;")
        return { children: children[0], parents: parents[0] }
      }),
    )

    expect(result.children).toHaveLength(1)
    expect(result.parents).toHaveLength(1)
  })

  it("does not turn an empty fallback field set into a first-row match", async () => {
    const Empty = Model.define("fallback_empty", Schema.Struct({}))
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const store = makeStore(Empty)
        yield* store.save({})
        yield* store.save({})
        const db = yield* Database
        const rows = yield* db.query("SELECT * FROM fallback_empty;")
        return rows[0]
      }),
    )

    expect(result).toHaveLength(2)
  })
})
