import { Effect, Schema } from "effect"
import { describe, expect, it } from "vitest"
import { field } from "../src/schema.js"
import { Model } from "../src/model.js"
import {
  decodeSqliteCursor,
  encodeSqliteCursor,
  makeSqliteStore,
  sqliteModel,
} from "../src/sqlite.js"
import { openSqliteClient } from "../src/sqlite-node.js"

const Entry = Schema.Struct({
  id: field(Schema.Int, { id: true }),
  createdAt: field(Schema.Int, { pagination: true }),
  body: Schema.String,
})
const model = Model.define("app_entry", Entry)

const run = <A,>(effect: Effect.Effect<A, unknown>) => Effect.runPromise(effect)

describe("SQLite store", () => {
  it("reuses model codecs for CRUD and bounded keyset pages", async () => {
    const handle = openSqliteClient(":memory:")
    try {
      const store = makeSqliteStore(handle.client, sqliteModel(model))
      await run(store.init)
      const entries = Array.from({ length: 5 }, (_, index) => ({
        id: index + 1,
        createdAt: index + 1,
        body: `entry-${index + 1}`,
      }))
      expect(await run(store.saveMany(entries))).toEqual(entries)
      expect(await run(store.list)).toEqual(entries)
      expect(await run(store.get(3))).toEqual(entries[2])

      const first = await run(store.paginAsc(2))
      expect(first.items).toEqual(entries.slice(0, 2))
      await expect(run(store.paginDesc(2, first.next))).rejects.toMatchObject({ kind: "InvalidModel" })
      const second = await run(store.paginAsc(2, first.next))
      expect(second.items).toEqual(entries.slice(2, 4))
      const last = await run(store.paginAsc(2, second.next))
      expect(last.items).toEqual(entries.slice(4))
      expect(last.next).toBeUndefined()

      const descending = await run(store.paginDesc(2))
      expect(descending.items).toEqual(entries.slice().reverse().slice(0, 2))

      const third = entries[2]!
      expect(await run(store.update({ ...third, body: "updated" }))).toEqual({ ...third, body: "updated" })
      expect(await run(store.get(3))).toEqual({ ...third, body: "updated" })
      expect(await run(store.delete(1))).toBe(true)
      expect(await run(store.delete(1))).toBe(false)
    } finally {
      handle.close()
    }
  })

  it("persists a cursor without exposing SQLite internals", () => {
    const cursor = { table: "app_entry", order: "asc" as const, orderKey: new Uint8Array([0x11, 0x80]), id: new Uint8Array([0x02, 0x80]) }
    expect(decodeSqliteCursor(encodeSqliteCursor(cursor))).toEqual(cursor)
  })
})
