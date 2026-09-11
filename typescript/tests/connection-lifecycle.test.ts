import { Cause, Effect, Exit, Fiber } from "effect"
import { createNodeEngines } from "@surrealdb/node"
import { createRemoteEngines, Surreal } from "surrealdb"
import { describe, expect, it } from "vitest"

import { Database, makeDatabaseLayer } from "../src/connection.js"

type Event = "query-start" | "query-settle" | "close-start" | "close-end"

const deferred = <A>() => {
  let resolve!: (value: A | PromiseLike<A>) => void
  const promise = new Promise<A>((next) => {
    resolve = next
  })
  return { promise, resolve }
}

const instrumentClient = (
  events: Event[],
  started: { resolve: () => void },
  settled: { resolve: () => void },
): Surreal => {
  const target = new Surreal({
    engines: {
      ...createRemoteEngines(),
      ...createNodeEngines(),
    },
  })

  return new Proxy(target, {
    get(inner, property) {
      if (property === "query") {
        return (...args: [string, Record<string, unknown>?]) => {
          events.push("query-start")
          const query = inner.query(...args)
          return new Proxy(query, {
            get(queryInner, queryProperty) {
              if (queryProperty === "responses") {
                return (...responseArgs: unknown[]) => {
                  const responses = queryInner.responses as (...values: unknown[]) => Promise<unknown>
                  const promise = responses.apply(queryInner, responseArgs)
                  started.resolve()
                  promise.then(
                    () => {
                      events.push("query-settle")
                      settled.resolve()
                    },
                    () => {
                      events.push("query-settle")
                      settled.resolve()
                    },
                  )
                  return promise
                }
              }
              return Reflect.get(queryInner, queryProperty, queryInner)
            },
          })
        }
      }
      if (property === "close") {
        return async (...args: []) => {
          events.push("close-start")
          const result = await inner.close(...args)
          events.push("close-end")
          return result
        }
      }
      const value = Reflect.get(inner, property, inner)
      return typeof value === "function" ? value.bind(inner) : value
    },
  }) as unknown as Surreal
}

describe("Database lifecycle", () => {
  it("settles an in-flight native query before Scope closes the engine", async () => {
    const events: Event[] = []
    const started = deferred<void>()
    const settled = deferred<void>()
    const client = instrumentClient(events, started, settled)
    const layer = makeDatabaseLayer({ endpoint: "mem://", makeClient: () => client })
    const program = Effect.scoped(
      Effect.provide(
        Effect.gen(function* () {
          const database = yield* Database
          return yield* database.query("SLEEP 100ms; RETURN 'done';")
        }),
        layer,
      ),
    )

    const fiber = Effect.runFork(program)
    await started.promise
    await Effect.runPromise(Fiber.interrupt(fiber))
    const exit = await Effect.runPromise(Effect.exit(Fiber.join(fiber)))
    await settled.promise

    if (!Exit.isFailure(exit)) throw new Error("expected interrupted query fiber to fail")
    expect(Cause.hasInterrupts(exit.cause)).toBe(true)
    expect(events).toEqual(["query-start", "query-settle", "close-start", "close-end"])
  })
})
