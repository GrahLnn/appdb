import { Effect, Exit, Schema } from "effect"
import { Surreal } from "surrealdb"
import { describe, expect, it } from "vitest"

import { Database, makeDatabaseLayer } from "../src/connection.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { field } from "../src/schema.js"

const StartupModel = Model.define(
  "schema_startup_user",
  Schema.Struct({
    id: field(RootIdSchema, { id: true }),
    email: field(Schema.String, { unique: true }),
    createdAt: field(Schema.Int, { pagination: true }),
  }),
)

const withSchemaDatabase = <A, E>(
  effect: Effect.Effect<A, E, Database>,
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "schema-startup",
          database: "schema-startup",
          schema: { models: [StartupModel] },
        }),
      ),
    ),
  )

describe("schema startup boundary", () => {
  it("applies model indexes before publishing the Database service", async () => {
    const observed = await withSchemaDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        const duplicate = yield* database.queryUnchecked(
          "CREATE schema_startup_user:one CONTENT { email: 'same@example.test', createdAt: 1 }; CREATE schema_startup_user:two CONTENT { email: 'same@example.test', createdAt: 2 };",
        )
        const info = yield* database.query("INFO FOR TABLE schema_startup_user;")
        return { duplicate, info }
      }),
    )

    expect(observed.duplicate[0]).toMatchObject({ success: true })
    expect(observed.duplicate[1]).toMatchObject({ success: false })
    const infoText = JSON.stringify(observed.info[0])
    expect(infoText).toContain("schema_startup_user_email_unique")
    expect(infoText).toContain("schema_startup_user_createdAt_id_pagin")
  })

  it("closes the acquired client and withholds Database when schema apply fails", async () => {
    let closes = 0
    let published = false
    const failingClient = {
      connect: async () => undefined,
      query: () => ({
        responses: async () => [
          {
            success: false,
            error: { name: "SchemaApplyError", message: "schema apply failed" },
          },
        ],
      }),
      close: async () => {
        closes += 1
      },
    } as unknown as Surreal

    const layer = makeDatabaseLayer({
      endpoint: "mem://",
      makeClient: () => failingClient,
      schema: { rawDdl: ["DEFINE TABLE schema_startup_failure SCHEMAFULL;"] },
    })
    const program = Effect.provide(
      Effect.gen(function* () {
        published = true
        yield* Database
      }),
      layer,
    )

    const exit = await Effect.runPromiseExit(Effect.scoped(program))

    expect(Exit.isFailure(exit)).toBe(true)
    expect(published).toBe(false)
    expect(closes).toBe(1)
  })
})
