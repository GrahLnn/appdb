import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Effect, Exit, Option } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { makeNodeDatabaseLayer, nodeEndpoint } from "../src/node.js"

describe("native engine persistence", () => {
  it.each(["structured", "raw"] as const)("releases the disk datastore at its requested path (%s)", async (style) => {
    const directory = await mkdtemp(join(tmpdir(), "appdb-native-persistence-"))
    const layer = () => makeNodeDatabaseLayer({
      endpoint: style === "structured"
        ? nodeEndpoint("surrealkv", join(directory, "data"))
        : `surrealkv://${join(directory, "data").replaceAll("\\", "/")}`,
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
      expect((await readdir(join(directory, "data"))).length).toBeGreaterThan(0)
      const result = await run(Effect.gen(function* () {
        const db = yield* Database
        return yield* db.query("SELECT VALUE value FROM persisted:one;")
      }))
      expect(result).toEqual([["saved"]])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  it.each(["structured", "raw"] as const)("rejects a native startup failure without leaving connect pending (%s)", async (style) => {
    const directory = await mkdtemp(join(tmpdir(), "appdb-native-failure-"))
    const blocker = join(directory, "file")
    await writeFile(blocker, "not a directory")
    const layer = makeNodeDatabaseLayer({
      endpoint: style === "structured"
        ? nodeEndpoint("surrealkv", join(blocker, "data"))
        : `surrealkv://${join(blocker, "data").replaceAll("\\", "/")}`,
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
      expect(Option.getOrUndefined(Exit.findErrorOption(exit))).toMatchObject({
        operation: "connect",
      })
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }, 5_000)
})
