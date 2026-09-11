import { Context, Effect, Layer, Schema, SchemaGetter } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { RootIdSchema } from "../src/id.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { paginate, type Page, type PageCursorInput } from "../src/pagination.js"
import { Model } from "../src/model.js"
import { makeStore } from "../src/repository.js"
import { field } from "../src/schema.js"

interface PaginationCodecState {
  readonly prefix: string
}

class PaginationCodecEnv extends Context.Service<
  PaginationCodecEnv,
  PaginationCodecState
>()("appdb/tests/PaginationCodecEnv") {}

const encodedRank = Schema.String.pipe(
  Schema.decodeTo(Schema.Number, {
    decode: SchemaGetter.transformEffect<number, string, PaginationCodecEnv>((encoded) =>
      PaginationCodecEnv.use((env) => Effect.succeed(Number(encoded.slice(env.prefix.length)))),
    ),
    encode: SchemaGetter.transformEffect<string, number, PaginationCodecEnv>((decoded) =>
      PaginationCodecEnv.use((env) => Effect.succeed(`${env.prefix}${decoded}`)),
    ),
  }),
)

const Rows = Model.define(
  "pagination_codec_rows",
  Schema.Struct({
    id: field(RootIdSchema, { id: true }),
    rank: field(encodedRank, { pagination: true }),
  }),
)

const withDatabase = <A, E>(effect: Effect.Effect<A, E, Database | PaginationCodecEnv>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        Effect.provide(
          effect,
          Layer.succeed(PaginationCodecEnv, { prefix: "n:" }),
        ),
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "pagination-codec",
          database: "pagination-codec",
        }),
      ),
    ),
  )

describe("pagination at the native schema boundary", () => {
  it("uses encoded values for keyset filtering before hydrating decoded rows", async () => {
    const store = makeStore(Rows)
    const result = await withDatabase(
      Effect.gen(function* () {
        yield* store.createAt("two", { id: "two", rank: 2 })
        yield* store.createAt("ten", { id: "ten", rank: 10 })
        yield* store.createAt("eleven", { id: "eleven", rank: 11 })

        const database = yield* Database
        const raw = yield* database.query(
          "SELECT record::id(id) AS id, rank FROM pagination_codec_rows ORDER BY rank ASC;",
        )
        const pages = []
        let cursor: PageCursorInput | undefined = undefined
        for (let index = 0; index < 3; index += 1) {
          const page: Page<typeof Rows.schema["Type"]> = yield* paginate(Rows, 1, cursor, "asc")
          pages.push(page)
          cursor = page.next
        }
        return { raw, pages }
      }),
    )

    expect(result.raw[0]).toEqual([
      { id: "ten", rank: "n:10" },
      { id: "eleven", rank: "n:11" },
      { id: "two", rank: "n:2" },
    ])
    expect(result.pages.flatMap((page) => page.items.map((item) => item.rank))).toEqual([10, 11, 2])
    expect(result.pages.map((page) => page.next !== undefined)).toEqual([true, true, false])
  })
})
