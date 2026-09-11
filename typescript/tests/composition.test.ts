import { Effect, Schema, SchemaGetter } from "effect"
import { describe, expect, it } from "vitest"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { Database } from "../src/connection.js"
import { makeStore } from "../src/repository.js"
import { Model } from "../src/model.js"
import { field } from "../src/schema.js"
import { RootIdSchema } from "../src/id.js"

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeNodeDatabaseLayer({ endpoint: "mem://", namespace: "app", database: "app" }))))

describe("composition probe", () => {
  it("decodes a foreign child exactly once when the child schema has a codec", async () => {
    const ChildSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      value: Schema.String.pipe(
        Schema.decodeTo(Schema.Number, {
          decode: SchemaGetter.transform((s) => Number(s)),
          encode: SchemaGetter.transform((n) => String(n)),
        }),
      ),
    })
    const Child = Model.define("child", ChildSchema)
    const ParentSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      child: field(ChildSchema, { foreign: { target: () => Child } }),
    })
    const Parent = Model.define("parent", ParentSchema)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const parentStore = makeStore(Parent)
        return yield* parentStore.createAt("p1", { id: "p1", child: { id: "c1", value: 7 } })
      }),
    )

    expect(result).toEqual({ id: "p1", child: { id: "c1", value: 7 } })
  })

  it("keeps both the canonical id field and a custom id field at the consumer boundary", async () => {
    const Canonical = Model.define(
      "canonical_id",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Custom = Model.define(
      "custom_id",
      Schema.Struct({
        key: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const canonical = makeStore(Canonical)
        const custom = makeStore(Custom)
        const canonicalValue = yield* canonical.createAt("one", { id: "one", label: "canonical" })
        const customValue = yield* custom.createAt("one", { key: "one", label: "custom" })
        return { canonicalValue, customValue }
      }),
    )

    expect(result).toEqual({
      canonicalValue: { id: "one", label: "canonical" },
      customValue: { key: "one", label: "custom" },
    })
  })
})
