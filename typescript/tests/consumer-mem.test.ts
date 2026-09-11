import { Effect, Schema } from "effect"
import { RecordId } from "surrealdb"
import { describe, expect, it } from "vitest"
import { Database } from "../src/connection.js"
import { makeStaticKeyLayer, Crypto, type KeyContext } from "../src/crypto.js"
import { I64_MAX, RootIdSchema, makeRecordId } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import {
  buildModelQuery,
  query,
} from "../src/query.js"
import {
  backRelateStatement,
  incomingIdsStatement,
  outgoingCountStatement,
  outgoingIdsStatement,
  outgoingRowsStatement,
  relateStatement,
  unrelateStatement,
} from "../src/relation.js"
import { field } from "../src/schema.js"

const makeMemLayer = () =>
  makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "consumer",
    database: "consumer",
  })

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeMemLayer())))

const UserSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  email: field(Schema.String, { unique: true }),
  createdAt: field(Schema.Int, { pagination: true }),
  name: Schema.String,
})

const User = Model.define("user", UserSchema)

describe("consumer contract on the embedded mem backend", () => {
  it("performs CRUD through the declared model identity and preserves unique conflicts", async () => {
    expect(User.recordId("one").table.name).toBe("user")
    expect(User.recordId("one").id).toBe("one")
    expect(User.recordId(I64_MAX).id).toBe(I64_MAX)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE user:one CONTENT { email: $email, createdAt: $createdAt, name: $name };",
          { email: "one@example.test", createdAt: 1, name: "One" },
        )
        yield* database.query(
          "UPDATE user:one MERGE { name: $name };",
          { name: "Updated" },
        )
        const duplicate = yield* database.queryUnchecked(
          "CREATE user:one CONTENT { email: $email, createdAt: $createdAt, name: $name };",
          { email: "one@example.test", createdAt: 2, name: "Two" },
        )
        const afterDuplicate = yield* database.query(
          "SELECT record::id(id) AS id, email, createdAt, name FROM user ORDER BY createdAt ASC;",
        )
        const deleted = yield* database.query("DELETE user:one RETURN BEFORE;")
        return { duplicate, afterDuplicate, deleted }
      }),
    )

    expect(result.duplicate[0]).toMatchObject({ success: false })
    expect(result.afterDuplicate[0]).toHaveLength(1)
    expect(result.afterDuplicate[0]).toEqual([
      { id: "one", email: "one@example.test", createdAt: 1, name: "Updated" },
    ])
    expect(result.deleted[0]).toHaveLength(1)
  })

  it("uses the bounded model query builder for ordered cursor pages and schema decoding", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE user:one CONTENT { email: 'one@example.test', createdAt: 1, name: 'One' }; CREATE user:two CONTENT { email: 'two@example.test', createdAt: 2, name: 'Two' }; CREATE user:three CONTENT { email: 'three@example.test', createdAt: 3, name: 'Three' };",
        )
        const statement = buildModelQuery({
          table: User.table,
          fields: User.fieldNames,
          predicate: { field: "createdAt", op: "gte", value: 1 },
          orderBy: { field: "createdAt", order: "asc" },
          limit: 2,
          offset: 1,
        })
        const slots = yield* query(statement)
        const rows = slots[0] as readonly unknown[]
        const decoded = yield* User.decode(rows[0])
        return { statement, rows, decoded }
      }),
    )

    expect(result.statement).toBeDefined()
    expect(result.rows).toHaveLength(2)
    expect(result.rows.map((row) => (row as { id: string }).id)).toEqual(["two", "three"])
    expect(result.decoded).toMatchObject({ id: "two", createdAt: 2, name: "Two" })
  })

  it("keeps foreign record identity and view ownership observable at the consumer boundary", async () => {
    const Child = Model.define(
      "child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const foreignTarget = () => Child
    const Parent = Model.define(
      "parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Schema.Unknown, { foreign: { target: foreignTarget } }),
        label: Schema.String,
      }),
    )
    const Summary = Model.view(
      Parent,
      Schema.Struct({ id: RootIdSchema, label: Schema.String }),
    )

    expect(Parent.fields.find((entry) => entry.path.join(".") === "child")?.metadata.foreign?.target).toBe(
      foreignTarget,
    )
    expect(Summary.source.kind).toBe("view")
    if (Summary.source.kind !== "view") throw new Error("expected a view model")
    expect(Summary.source.owner).toBe(Parent)
    expect(Summary.source.ownerIdentity).toBe(Parent.source.ownerIdentity)
    expect(Summary.table).toBe("parent")

    const foreign = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("CREATE child:c1 CONTENT { label: 'Child' }; CREATE parent:p1 CONTENT { child: child:c1, label: 'Parent' };")
        const slots = yield* database.query(
          "SELECT child, record::tb(child) AS child_table, label FROM parent:p1;",
        )
        return slots[0] as readonly { child: unknown; child_table: unknown; label: unknown }[]
      }),
    )

    expect(foreign).toHaveLength(1)
    expect(foreign[0]?.child).toBeInstanceOf(RecordId)
    expect((foreign[0]?.child as RecordId).table.name).toBe("child")
    expect(foreign[0]?.child_table).toBe("child")
    expect(foreign[0]?.label).toBe("Parent")
  })

  it("writes and reads graph edges through the shared relation statements", async () => {
    const alice = makeRecordId("person", "alice")
    const bob = makeRecordId("person", "bob")
    const carol = makeRecordId("person", "carol")

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("CREATE person:alice CONTENT { name: 'Alice' }; CREATE person:bob CONTENT { name: 'Bob' }; CREATE person:carol CONTENT { name: 'Carol' };")
        yield* database.query(relateStatement(alice, bob, "likes"))
        yield* database.query(backRelateStatement(alice, carol, "likes"))
        const outgoingIds = yield* database.query(outgoingIdsStatement(alice, "likes"))
        const outgoingCount = yield* database.query(outgoingCountStatement(alice, "likes"))
        const outgoingRows = yield* database.query(outgoingRowsStatement(alice, "likes"))
        const incomingIds = yield* database.query(incomingIdsStatement(alice, "likes"))
        yield* database.query(unrelateStatement(alice, bob, "likes"))
        const countAfterDelete = yield* database.query(outgoingCountStatement(alice, "likes"))
        return { outgoingIds, outgoingCount, outgoingRows, incomingIds, countAfterDelete }
      }),
    )

    expect(result.outgoingIds[0]).toEqual([bob])
    expect(result.outgoingCount[0]).toEqual(1)
    expect(result.outgoingRows[1]).toEqual([{ id: "bob", name: "Bob" }])
    expect(result.incomingIds[0]).toEqual([carol])
    expect(result.countAfterDelete[0]).toEqual(0)
  })

  it("rolls back every write in a failed explicit transaction while retaining earlier data", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("CREATE tx:prior CONTENT { marker: 0 };")
        const responses = yield* database.queryUnchecked(
          "BEGIN TRANSACTION; CREATE tx:written CONTENT { marker: 1 }; THROW 'abort';",
        )
        const rows = yield* database.query("SELECT record::id(id) AS id, marker FROM tx ORDER BY id ASC;")
        return { responses, rows }
      }),
    )

    expect(result.responses.some((response) => response.success === false)).toBe(true)
    expect(result.rows[0]).toEqual([{ id: "prior", marker: 0 }])
  })

  it("round-trips a fixed-key ciphertext through the same database consumer", async () => {
    const context: KeyContext = { model: "Secret", field: "token" }
    const fixedKey = new Uint8Array(Array.from({ length: 32 }, (_, index) => index + 1))
    const program = Effect.gen(function* () {
      const database = yield* Database
      const crypto = yield* Crypto
      const ciphertext = yield* crypto.encryptText("consumer-secret", context)
      yield* database.query("CREATE secret:one CONTENT { payload: $payload };", {
        payload: Array.from(ciphertext),
      })
      const slots = yield* database.query("SELECT payload FROM secret:one;")
      const row = (slots[0] as readonly { payload: unknown }[])[0]
      const stored = row?.payload
      const bytes = stored instanceof Uint8Array
        ? stored
        : Uint8Array.from(stored as readonly number[])
      return { ciphertext, plaintext: yield* crypto.decryptText(bytes, context) }
    })
    const result = await Effect.runPromise(
      Effect.scoped(
        Effect.provide(
          Effect.provide(program, makeMemLayer()),
          makeStaticKeyLayer(fixedKey),
        ),
      ),
    )

    expect(result.ciphertext).toHaveLength(12 + "consumer-secret".length + 16)
    expect(result.plaintext).toBe("consumer-secret")
  })
})
