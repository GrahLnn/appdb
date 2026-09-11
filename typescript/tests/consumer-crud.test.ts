import { Effect, Exit, Option, Schema } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { DbErrorKind } from "../src/errors.js"
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
          namespace: "consumer-crud",
          database: "consumer-crud",
        }),
      ),
    ),
  )

describe("public Store CRUD controls", () => {
  it("reports createAt conflict and preserves the existing row", async () => {
    const Item = Model.define(
      "consumer_crud_create_at",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const store = makeStore(Item)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        yield* store.createAt("one", { id: "one", label: "first" })
        const conflict = yield* Effect.exit(
          store.createAt("one", { id: "one", label: "second" }),
        )
        const existing = yield* store.get("one")
        return { conflict, existing }
      }),
    )

    expect(Exit.isFailure(result.conflict)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.conflict))).toMatchObject({
      kind: DbErrorKind.Conflict,
    })
    expect(result.existing).toEqual({ id: "one", label: "first" })
  })

  it("reports updateAt on a missing record without creating one", async () => {
    const Item = Model.define(
      "consumer_crud_update_at",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const store = makeStore(Item)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("DEFINE TABLE IF NOT EXISTS consumer_crud_update_at SCHEMALESS;")
        const missing = yield* Effect.exit(
          store.updateAt("missing", { id: "missing", label: "never-created" }),
        )
        const rows = yield* store.list()
        return { missing, rows }
      }),
    )

    expect(Exit.isFailure(result.missing)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.missing))).toMatchObject({
      kind: DbErrorKind.NotFound,
    })
    expect(result.rows).toEqual([])
  })

  it("uses upsertAt for both new and existing ids with REPLACE semantics", async () => {
    const Item = Model.define(
      "consumer_crud_upsert_at",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
        stale: Schema.optionalKey(Schema.String),
      }),
    )
    const store = makeStore(Item)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const created = yield* store.upsertAt("one", {
          id: "one",
          label: "first",
          stale: "remove-on-replace",
        })
        const replaced = yield* store.upsertAt("one", { id: "one", label: "second" })
        const fetched = yield* store.get("one")
        return { created, replaced, fetched }
      }),
    )

    expect(result.created).toEqual({
      id: "one",
      label: "first",
      stale: "remove-on-replace",
    })
    expect(result.replaced).toEqual({ id: "one", label: "second" })
    expect(result.fetched).toEqual({ id: "one", label: "second" })
  })

  it("reuses the existing record id when save resolves a unique value", async () => {
    const Item = Model.define(
      "consumer_crud_unique_save",
      Schema.Struct({
        email: field(Schema.String, { unique: true }),
        label: Schema.String,
      }),
    )
    const store = makeStore(Item)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("DEFINE TABLE IF NOT EXISTS consumer_crud_unique_save SCHEMALESS;")
        const first = yield* store.save({ email: "same@example.com", label: "first" })
        const firstIds = yield* store.listRecordIds()
        const second = yield* store.save({ email: "same@example.com", label: "second" })
        const secondIds = yield* store.listRecordIds()
        return { first, second, firstIds, secondIds }
      }),
    )

    expect(result.first).toEqual({ email: "same@example.com", label: "first" })
    expect(result.second).toEqual({ email: "same@example.com", label: "second" })
    expect(result.firstIds).toHaveLength(1)
    expect(result.secondIds).toHaveLength(1)
    expect(String(result.secondIds[0])).toBe(String(result.firstIds[0]))
  })

  it("rolls back a new foreign child when createAt conflicts on an existing parent", async () => {
    const Child = Model.define(
      "consumer_crud_foreign_child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Parent = Model.define(
      "consumer_crud_foreign_parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Child.schema, { foreign: { target: () => Child } }),
      }),
    )
    const store = makeStore(Parent)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        yield* store.createAt("parent", {
          id: "parent",
          child: { id: "old-child", label: "old" },
        })
        const conflict = yield* Effect.exit(
          store.createAt("parent", {
            id: "parent",
            child: { id: "new-child", label: "new" },
          }),
        )
        const parent = yield* store.get("parent")
        const database = yield* Database
        const children = yield* database.query(
          "SELECT record::id(id) AS id FROM consumer_crud_foreign_child ORDER BY id ASC;",
        )
        return { conflict, parent, children }
      }),
    )

    expect(Exit.isFailure(result.conflict)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.conflict))).toMatchObject({
      kind: DbErrorKind.Conflict,
    })
    expect(result.parent).toEqual({
      id: "parent",
      child: { id: "old-child", label: "old" },
    })
    expect(result.children[0]).toEqual([{ id: "old-child" }])
  })

  it("keeps merge and patch rejected for a rich foreign model", async () => {
    const Child = Model.define(
      "consumer_crud_rich_child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Parent = Model.define(
      "consumer_crud_rich_parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Child.schema, { foreign: { target: () => Child } }),
      }),
    )
    const store = makeStore(Parent)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        yield* store.createAt("parent", {
          id: "parent",
          child: { id: "child", label: "kept" },
        })
        const merged = yield* Effect.exit(store.merge("parent", { child: { id: "other" } }))
        const patched = yield* Effect.exit(store.patch("parent", []))
        return { merged, patched }
      }),
    )

    expect(Exit.isFailure(result.merged)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.merged))).toMatchObject({
      kind: DbErrorKind.InvalidModel,
    })
    expect(Exit.isFailure(result.patched)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(result.patched))).toMatchObject({
      kind: DbErrorKind.InvalidModel,
    })
  })
})
