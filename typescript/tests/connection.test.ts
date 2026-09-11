import { Effect, Exit } from "effect"
import { Surreal } from "surrealdb"
import { describe, expect, it } from "vitest"
import {
  Database,
  makeDatabaseLayer,
} from "../src/connection.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { DbError } from "../src/errors.js"

const makeMemLayer = () =>
  makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "app",
    database: "app",
  })

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeMemLayer())))

describe("Database connection service", () => {
  it("keeps checked query slots and parameter bindings", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        return yield* database.query(
          "BEGIN TRANSACTION; CREATE checked:one CONTENT { marker: $marker }; COMMIT TRANSACTION;",
          { marker: 7 },
        )
      }),
    )

    expect(result).toHaveLength(3)
    expect(result[0]).toBeUndefined()
    expect(result[1]).toEqual([{ id: expect.anything(), marker: 7 }])
    expect(result[2]).toBeUndefined()
  })

  it("preserves own undefined and null values in checked raw projections", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        return yield* database.query(
          "CREATE checked_projection:one CONTENT { nullable: null }; SELECT missing, nullable FROM checked_projection:one;",
        )
      }),
    )

    const rows = result[1]
    expect(Array.isArray(rows)).toBe(true)
    const row = (rows as readonly unknown[])[0] as Record<string, unknown>
    expect(Object.hasOwn(row, "missing")).toBe(true)
    expect(row.missing).toBeUndefined()
    expect(Object.hasOwn(row, "nullable")).toBe(true)
    expect(row.nullable).toBeNull()
  })

  it("retains per-statement failures through queryUnchecked", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        return yield* database.queryUnchecked(
          "BEGIN TRANSACTION; CREATE unchecked:one CONTENT { marker: $marker }; THROW 'rollback'; COMMIT TRANSACTION;",
          { marker: 9 },
        )
      }),
    )

    expect(result).toHaveLength(3)
    expect(result[0]).toMatchObject({ success: true, result: undefined })
    expect(result[1]).toMatchObject({ success: false, error: { name: "QueryError" } })
    expect(result[2]).toMatchObject({ success: false, error: { name: "ThrownError" } })
  })

  it("maps checked statement errors into the shared DbError", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        return yield* Effect.flip(database.query("THROW 'checked failure'"))
      }),
    )

    expect(result).toBeInstanceOf(DbError)
    expect(result.kind).toBe("Engine")
    expect(result.operation).toBe("query")
  })

  it("closes the underlying client when the Layer scope ends", async () => {
    let client: Surreal | undefined
    await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        client = database.client
        yield* database.query("CREATE lifecycle:one CONTENT { ok: true }")
      }),
    )

    expect(client?.isConnected).toBe(false)
  })

  it("registers close before a startup failure", async () => {
    let closes = 0
    const failingClient = {
      connect: async () => {
        throw new Error("startup failed")
      },
      close: async () => {
        closes += 1
        return true
      },
    } as unknown as Surreal
    const layer = makeDatabaseLayer({
      endpoint: "mem://",
      makeClient: () => failingClient,
    })
    const program = Effect.provide(
      Effect.gen(function* () {
        const database = yield* Database
        return yield* database.query("SELECT 1")
      }),
      layer,
    )

    const exit = await Effect.runPromiseExit(Effect.scoped(program))

    expect(Exit.isFailure(exit)).toBe(true)
    expect(closes).toBe(1)
  })
})
