import { Schema } from "effect"
import { RecordId } from "surrealdb"
import { describe, expect, it } from "vitest"

import { DbError } from "../src/errors.js"
import { I64_MAX, RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { PageCursor, PaginationPlan } from "../src/pagination.js"
import { field } from "../src/schema.js"

const Post = Model.define(
  "post",
  Schema.Struct({
    id: field(RootIdSchema, { id: true }),
    createdAt: field(Schema.Int, { pagination: true }),
  }),
)

describe("keyset pagination contract", () => {
  it("keeps table, field, order, scalar precision, and full record identity in a cursor", () => {
    const plan = new PaginationPlan(Post, "createdAt", "asc")
    const cursor = plan.buildCursor({ id: I64_MAX, createdAt: 7 })
    const decoded = PageCursor.fromString(cursor.asString())

    expect(decoded.table).toBe("post")
    expect(decoded.field).toBe("createdAt")
    expect(decoded.order).toBe("asc")
    expect(decoded.value).toBe(7)
    expect(decoded.id).toBeInstanceOf(RecordId)
    expect((decoded.id as RecordId).table.name).toBe("post")
    expect((decoded.id as RecordId).id).toBe(I64_MAX)

    const statement = plan.buildStatement(3, decoded)
    expect(statement.query).toContain("__page_record > $cursor_record")
    expect(statement.query).toContain("ORDER BY createdAt ASC, __page_record ASC LIMIT $count")
    expect(statement.bindings.cursor_record).toBeInstanceOf(RecordId)
  })

  it("rejects a cursor when its model boundary or direction changes", () => {
    const cursor = new PaginationPlan(Post, "createdAt", "asc").buildCursor({ id: "p1", createdAt: 1 })
    const descending = new PaginationPlan(Post, "createdAt", "desc")
    expect(() => descending.buildStatement(2, cursor)).toThrow(DbError)

    const Other = Model.define(
      "other",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        createdAt: field(Schema.Int, { pagination: true }),
      }),
    )
    expect(() => new PaginationPlan(Other, "createdAt", "asc").buildStatement(2, cursor)).toThrow(DbError)
  })

  it("encodes bigint cursor values without converting them to a number", () => {
    const cursor = PageCursor.create({
      table: "post",
      field: "createdAt",
      order: "desc",
      value: 9223372036854775807n,
      id: new RecordId("post", "p1"),
    })
    const decoded = PageCursor.decode(cursor.toString())
    expect(decoded.value).toBe(9223372036854775807n)
  })
})

