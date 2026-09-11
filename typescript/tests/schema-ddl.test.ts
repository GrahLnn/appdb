import { Cause, Effect, Exit, Schema } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { DbError, DbErrorKind } from "../src/errors.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { Model } from "../src/model.js"
import { field } from "../src/schema.js"
import { RootIdSchema } from "../src/id.js"
import {
  applySchema,
  hnswIndexDdl,
  modelSchemaDdl,
  paginationIndexDdl,
  schemaDdl,
  tableBootstrapDdl,
  uniqueIndexDdl,
} from "../src/schema-ddl.js"

const makeMemLayer = () =>
  makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "schema_ddl",
    database: "schema_ddl",
  })

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(Effect.scoped(Effect.provide(effect, makeMemLayer())))

const User = Model.define(
  "schema_ddl_user",
  Schema.Struct({
    id: field(RootIdSchema, { id: true }),
    email: field(Schema.String, { unique: true }),
    createdAt: field(Schema.Int, { pagination: true }),
    label: Schema.String,
  }),
)

const AliasId = Model.define(
  "schema_ddl_alias",
  Schema.Struct({
    key: field(RootIdSchema, { id: true, pagination: true }),
    label: Schema.String,
  }),
)

const UserView = Model.view(
  User,
  Schema.Struct({ email: Schema.String }),
)

const SqlUser = Model.sqlView(
  "schema_ddl_sql_user",
  Schema.Struct({ email: Schema.String }),
  { params: Schema.Struct({}), sql: "SELECT email FROM schema_ddl_user" },
)

describe("schema DDL planning and application", () => {
  it("derives independent unique and pagination indexes with physical id mapping", () => {
    expect(tableBootstrapDdl(User.table)).toBe(
      "DEFINE TABLE IF NOT EXISTS schema_ddl_user SCHEMALESS;",
    )
    expect(uniqueIndexDdl(User.table, "email")).toBe(
      "DEFINE INDEX IF NOT EXISTS schema_ddl_user_email_unique ON schema_ddl_user FIELDS email UNIQUE;",
    )
    expect(paginationIndexDdl(User.table, "createdAt", User.idField)).toBe(
      "DEFINE INDEX IF NOT EXISTS schema_ddl_user_createdAt_id_pagin ON schema_ddl_user FIELDS createdAt,id;",
    )
    expect(modelSchemaDdl(User)).toEqual([
      tableBootstrapDdl(User.table),
      uniqueIndexDdl(User.table, "email"),
      paginationIndexDdl(User.table, "createdAt", User.idField),
    ])
    expect(modelSchemaDdl(AliasId)).toEqual([
      tableBootstrapDdl(AliasId.table),
      paginationIndexDdl(AliasId.table, "key", AliasId.idField),
    ])
    expect(modelSchemaDdl(AliasId)[1]).toContain("FIELDS id;")
  })

  it("preserves raw statement bytes and order while deduplicating generated DDL only", () => {
    const raw = "  DEFINE TABLE IF NOT EXISTS `schema_ddl_raw` SCHEMAFULL;  "
    expect(schemaDdl({ rawDdl: [raw, raw], models: [User] }).slice(0, 2)).toEqual([raw, raw])
    expect(schemaDdl({ models: [User, User] })).toEqual(modelSchemaDdl(User))
  })

  it("uses a table view's real Store owner and rejects SQL views", () => {
    expect(modelSchemaDdl(UserView)).toEqual(modelSchemaDdl(User))
    expect(() => modelSchemaDdl(SqlUser)).toThrowError(
      expect.objectContaining({ kind: DbErrorKind.InvalidModel }),
    )
  })

  it("renders the Rust-compatible minimal and configured HNSW forms", () => {
    expect(hnswIndexDdl({
      name: "schema_ddl_embedding_hnsw",
      table: "schema_ddl_vector",
      field: "embedding",
      dimension: 2,
    })).toBe(
      "DEFINE INDEX IF NOT EXISTS schema_ddl_embedding_hnsw ON schema_ddl_vector FIELDS embedding HNSW DIMENSION 2;",
    )
    expect(hnswIndexDdl({
      name: "schema_ddl_nested_hnsw",
      table: "schema_ddl_vector",
      field: "items.embedding",
      dimension: 64,
      vectorType: "F32",
      distance: "COSINE",
      efConstruction: 150,
      m: 12,
      concurrently: true,
      defer: true,
    })).toBe(
      "DEFINE INDEX IF NOT EXISTS schema_ddl_nested_hnsw ON schema_ddl_vector FIELDS items.embedding HNSW DIMENSION 64 TYPE F32 DIST COSINE EFC 150 M 12 CONCURRENTLY DEFER;",
    )
    expect(() => hnswIndexDdl({
      name: "schema_ddl_invalid_hnsw",
      table: "schema_ddl_vector",
      field: "embedding",
      dimension: 0,
    })).toThrow("HNSW dimension must be a positive safe integer")
  })

  it("applies generated indexes and exposes the physical table inventory", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* applySchema(database, { models: [User] })
        yield* database.query(
          "CREATE schema_ddl_user:one CONTENT { email: $email, createdAt: 1, label: 'one' };",
          { email: "one@example.test" },
        )
        const duplicate = yield* database.queryUnchecked(
          "CREATE schema_ddl_user:two CONTENT { email: $email, createdAt: 2, label: 'two' };",
          { email: "one@example.test" },
        )
        const info = yield* database.query("INFO FOR TABLE schema_ddl_user;")
        return { duplicate, info }
      }),
    )

    expect(result.duplicate[0]).toMatchObject({ success: false })
    const info = result.info[0] as { indexes?: Record<string, unknown> }
    const indexes = Object.values(info.indexes ?? {}).map(String)
    expect(indexes.some((ddl) => ddl.includes("FIELDS email UNIQUE"))).toBe(true)
    expect(indexes.some((ddl) => ddl.includes("FIELDS createdAt, id") && !ddl.includes("UNIQUE"))).toBe(true)
  })

  it("keeps explicit schemafull field DDL effective and repeatable", async () => {
    const definition = {
      rawDdl: [
        "DEFINE TABLE IF NOT EXISTS schema_ddl_strict SCHEMAFULL;",
        "DEFINE FIELD IF NOT EXISTS email ON schema_ddl_strict TYPE string;",
      ],
    } as const
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* applySchema(database, definition)
        yield* applySchema(database, definition)
        const invalidRow = yield* database.queryUnchecked(
          "CREATE schema_ddl_strict:wrong CONTENT { email: 17 };",
        )
        const info = yield* database.query("INFO FOR TABLE schema_ddl_strict;")
        return { invalidRow, info }
      }),
    )

    expect(result.invalidRow[0]).toMatchObject({ success: false })
    const info = result.info[0] as { fields?: Record<string, unknown> }
    expect(Object.keys(info.fields ?? {})).toContain("email")
  })

  it("applies the minimal HNSW definition and exposes it through INFO", async () => {
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* applySchema(database, {
          rawDdl: ["DEFINE TABLE IF NOT EXISTS schema_ddl_vector SCHEMALESS;"],
          hnsw: [{
            name: "schema_ddl_embedding_hnsw",
            table: "schema_ddl_vector",
            field: "embedding",
            dimension: 2,
          }],
        })
        return yield* database.query("INFO FOR TABLE schema_ddl_vector;")
      }),
    )

    const info = result[0] as { indexes?: Record<string, unknown> }
    const indexes = Object.values(info.indexes ?? {}).map(String)
    expect(indexes.some((ddl) => ddl.includes("HNSW DIMENSION 2"))).toBe(true)
  })

  it("stops before later statements when the schema executor fails", async () => {
    const calls: string[] = []
    const result = await Effect.runPromiseExit(
      applySchema(
        {
          query: (statement) => {
            calls.push(statement)
            return statement.includes("bad") ? Effect.fail("schema failed") : Effect.succeed(undefined)
          },
        },
        { rawDdl: ["DEFINE TABLE good;", "DEFINE TABLE bad;", "DEFINE TABLE after;"] },
      ),
    )

    expect(calls).toEqual(["DEFINE TABLE good;", "DEFINE TABLE bad;"])
    expect(result._tag).toBe("Failure")
  })

  it("returns pure DDL validation failures as typed Effect errors", async () => {
    const exit = await Effect.runPromiseExit(
      applySchema(
        { query: () => Effect.succeed(undefined) },
        {
          hnsw: [{
            name: "schema_ddl_invalid_hnsw",
            table: "schema_ddl_vector",
            field: "embedding",
            dimension: 0,
          }],
        },
      ),
    )

    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) {
      const failure = Cause.findFail(exit.cause)
      expect(failure._tag).toBe("Success")
      if (failure._tag === "Success") {
        expect(failure.success.error).toBeInstanceOf(DbError)
        expect(failure.success.error).toMatchObject({ kind: DbErrorKind.InvalidModel })
      }
    }
  })
})
