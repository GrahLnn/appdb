import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Effect } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { makeNodeDatabaseLayer, nodeEndpoint } from "../src/node.js"

describe("native engine persistence", () => {
  it("releases the disk datastore before Scope close returns", async () => {
    const directory = await mkdtemp(join(tmpdir(), "appdb-native-persistence-"))
    const layer = () => makeNodeDatabaseLayer({
      endpoint: nodeEndpoint("surrealkv", join(directory, "data")),
      namespace: "app",
      database: "app",
    })
    const run = <A, E>(program: Effect.Effect<A, E, Database>) =>
      Effect.runPromise(Effect.scoped(Effect.provide(program, layer())))
    try {
      await run(Effect.gen(function* () {
        const db = yield* Database
        yield* db.query("CREATE persisted:one CONTENT { value: 'saved' };")
      }))
      const result = await run(Effect.gen(function* () {
        const db = yield* Database
        return yield* db.query("SELECT VALUE value FROM persisted:one;")
      }))
      expect(result).toEqual([["saved"]])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  it("rejects a native startup failure without leaving connect pending", async () => {
    const directory = await mkdtemp(join(tmpdir(), "appdb-native-failure-"))
    const blocker = join(directory, "file")
    await writeFile(blocker, "not a directory")
    const layer = makeNodeDatabaseLayer({
      endpoint: nodeEndpoint("surrealkv", join(blocker, "data")),
      namespace: "app",
      database: "app",
    })
    try {
      const exit = await Effect.runPromise(
        Effect.scoped(Effect.provide(Effect.gen(function* () {
          yield* Database
        }), layer)).pipe(Effect.exit),
      )
      expect(exit._tag).toBe("Failure")
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }, 5_000)
})
