import { Effect, Fiber, Result } from "effect"
import { Surreal, type SurrealTransaction } from "surrealdb"
import { describe, expect, it } from "vitest"

import { Database, makeDatabaseLayer } from "../src/connection.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import {
  isTransactionCapabilityError,
  runTx,
  TransactionCapabilityError,
  TxStmt,
} from "../src/tx.js"

const makeMemLayer = () =>
  makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "tx-tests",
    database: "tx-tests",
  })

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeMemLayer())))

describe("transaction execution", () => {
  it("keeps the Rust empty-input result outside the transaction", async () => {
    const result = await withMemDatabase(runTx([]))

    expect(result.len()).toBe(1)
    expect(result.get(0)?.length).toBe(1)
    expect(result.get(0)?.get(0)).toMatchObject({ success: true })
  })

  it("keeps one response group per input, including multiple statements", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        return yield* runTx([
          new TxStmt(
            "CREATE tx:first CONTENT { marker: $marker }; SELECT marker FROM tx:first;",
            { marker: 0 },
          ).bind("marker", 1),
          new TxStmt("CREATE tx:second CONTENT { marker: $marker };", { marker: 2 }),
        ])
      }),
    )

    expect(result.len()).toBe(2)
    expect(result.get(0)?.length).toBe(2)
    expect(result.take<readonly { marker: number }[]>(0, 0)).toEqual([
      { id: expect.anything(), marker: 1 },
    ])
    expect(result.take<readonly { marker: number }[]>(0, 1)).toEqual([{ marker: 1 }])
    expect(result.take<readonly { marker: number }[]>(1, 0)).toEqual([
      { id: expect.anything(), marker: 2 },
    ])
  })

  it("cancels the transaction when a later input fails", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("CREATE tx:prior CONTENT { marker: 0 };")
        const outcome = yield* Effect.result(
          runTx([
            "CREATE tx:rolled_back CONTENT { marker: 1 };",
            "THROW 'abort';",
          ]),
        )
        const rows = yield* database.query(
          "SELECT record::id(id) AS id, marker FROM tx ORDER BY id ASC;",
        )
        return { outcome, rows }
      }),
    )

    expect(Result.isFailure(result.outcome)).toBe(true)
    expect(result.rows[0]).toEqual([{ id: "prior", marker: 0 }])
  })

  it("returns a typed capability error when beginTransaction is unavailable", async () => {
    const client = {
      connect: async () => true,
      close: async () => true,
    } as unknown as Surreal
    const layer = makeDatabaseLayer({
      endpoint: "fake://",
      makeClient: () => client,
    })

    const outcome = await Effect.runPromise(
      Effect.scoped(
        Effect.provide(
          Effect.result(runTx(["RETURN 1;"])),
          layer,
        ),
      ),
    )

    expect(Result.isFailure(outcome)).toBe(true)
    if (Result.isSuccess(outcome)) throw new Error("expected transaction capability failure")
    expect(outcome.failure).toBeInstanceOf(TransactionCapabilityError)
    expect(isTransactionCapabilityError(outcome.failure)).toBe(true)
  })

  it("waits for an in-flight transaction query before cancelling", async () => {
    let releaseQuery!: () => void
    const queryGate = new Promise<void>((resolve) => {
      releaseQuery = resolve
    })
    let queryStarted!: () => void
    const queryStartedSignal = new Promise<void>((resolve) => {
      queryStarted = resolve
    })
    let cancels = 0
    const events: string[] = []
    const transaction = {
      query: () => ({
        responses: async () => {
          events.push("query-start")
          queryStarted()
          await queryGate
          events.push("query-settle")
          return [{ success: true, result: [1] }]
        },
      }),
      commit: async () => undefined,
      cancel: async () => {
        cancels += 1
        events.push("cancel")
      },
    } as unknown as SurrealTransaction
    const client = {
      connect: async () => true,
      close: async () => true,
      beginTransaction: async () => transaction,
    } as unknown as Surreal
    const layer = makeDatabaseLayer({
      endpoint: "fake://",
      makeClient: () => client,
    })
    const fiber = Effect.runFork(
      Effect.scoped(Effect.provide(runTx(["RETURN 1;"]), layer)),
    )

    await queryStartedSignal
    events.push("interrupt-requested")
    const interrupt = Effect.runPromise(Fiber.interrupt(fiber))
    await Promise.resolve()
    expect(cancels).toBe(0)

    releaseQuery()
    await interrupt
    expect(cancels).toBe(1)
    expect(events).toEqual(["query-start", "interrupt-requested", "query-settle", "cancel"])
  })

})
