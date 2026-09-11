import { Effect, Schema } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeView } from "../src/repository.js"
import { field } from "../src/schema.js"

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "nested-view-projection",
          database: "nested-view-projection",
        }),
      ),
    ),
  )

describe("nested view projection", () => {
  it("projects an inline object while keeping its child metadata out of root roles", async () => {
    const ChildSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    })
    const Owner = Model.define(
      "nested_view_owner",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        inline: ChildSchema,
      }),
    )
    const View = Model.view(
      Owner,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        inline: ChildSchema,
      }),
    )

    expect(View.fieldNames).toEqual(["id", "inline"])
    expect(View.idField).toBe("id")
    expect(View.uniqueFields).toEqual([])
    expect(View.paginationField).toBeUndefined()

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE nested_view_owner:one CONTENT { inline: { id: 'child-1', label: 'nested' } };",
        )
        return yield* makeView(View).get("one")
      }),
    )

    expect(result).toEqual({
      id: "one",
      inline: { id: "child-1", label: "nested" },
    })
  })
})
