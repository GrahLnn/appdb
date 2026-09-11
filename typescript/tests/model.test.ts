import { Context, Effect, Schema, SchemaGetter } from "effect"
import { describe, expect, it } from "vitest"
import { DbError, DbErrorKind } from "../src/errors.js"
import { I64_MAX, I64_MIN, RootIdSchema, makeRecordId, rootIdFromRecord } from "../src/id.js"
import { Model } from "../src/model.js"
import { field, foreign, relation } from "../src/schema.js"

describe("native schema model contract", () => {
  it("keeps public ids scalar and checks safe number/i64 bounds", async () => {
    expect(Schema.decodeUnknownSync(RootIdSchema)("user-1")).toBe("user-1")
    expect(Schema.decodeUnknownSync(RootIdSchema)(Number.MAX_SAFE_INTEGER)).toBe(
      Number.MAX_SAFE_INTEGER,
    )
    expect(Schema.decodeUnknownSync(RootIdSchema)(I64_MIN)).toBe(I64_MIN)
    expect(Schema.decodeUnknownSync(RootIdSchema)(I64_MAX)).toBe(I64_MAX)
    expect(() => Schema.decodeUnknownSync(RootIdSchema)(Number.MAX_SAFE_INTEGER + 1)).toThrow()
    expect(() => Schema.decodeUnknownSync(RootIdSchema)(I64_MAX + 1n)).toThrow()

    const record = makeRecordId("user", "user-1")
    expect(rootIdFromRecord(record, "user")).toBe("user-1")
    expect(() => rootIdFromRecord(record, "other")).toThrowError(DbError)
  })

  it("derives one storage plan from native schema annotations", async () => {
    const UserSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      email: field(Schema.String, { unique: true }),
      createdAt: field(Schema.Int, { pagination: true }),
      profile: Schema.Struct({
        displayName: field(Schema.String, {
          sensitive: { scope: "leaf", keyContext: { model: "user", field: "displayName" } },
        }),
      }),
    })
    const User = Model.define("user", UserSchema)

    expect(User.fieldNames).toEqual(["id", "email", "createdAt", "profile"])
    expect(User.idField).toBe("id")
    expect(User.uniqueFields).toEqual(["email"])
    expect(User.paginationField).toBe("createdAt")
    expect(User.fields.some((entry) => entry.path.join(".") === "profile.displayName")).toBe(true)

    const value = await Effect.runPromise(
      User.decode({ id: "user-1", email: "a@example.com", createdAt: 1, profile: { displayName: "A" } }),
    )
    expect(value.profile.displayName).toBe("A")
    expect(await Effect.runPromise(User.encode(value))).toEqual({
      id: "user-1",
      email: "a@example.com",
      createdAt: 1,
      profile: { displayName: "A" },
    })
  })

  it("preserves lazy target identity and rejects incompatible field roles", () => {
    let targetCalls = 0
    const target = () => {
      targetCalls += 1
      return Model.define("child", Schema.Struct({ id: RootIdSchema }))
    }
    const Parent = Model.define(
      "parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Schema.Struct({ id: RootIdSchema }), { foreign: { target } }),
      }),
    )
    expect(Parent.fields.find((entry) => entry.path.join(".") === "child")?.metadata.foreign?.target).toBe(
      target,
    )
    expect(targetCalls).toBe(0)

    expect(() =>
      Model.define(
        "bad",
        Schema.Struct({
          secret: field(Schema.String, {
            unique: true,
            sensitive: { scope: "leaf" },
          }),
        }),
      ),
    ).toThrowError(DbError)
    try {
      Model.define(
        "bad",
        Schema.Struct({
          secret: field(Schema.String, {
            pagination: true,
            sensitive: { scope: "leaf" },
          }),
        }),
      )
    } catch (error) {
      expect(error).toMatchObject({ kind: DbErrorKind.InvalidModel })
    }
  })

  it("builds foreign and relation schemas from lazy target models", () => {
    const Child = Model.define(
      "child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    let targetCalls = 0
    const target = () => {
      targetCalls += 1
      return Child
    }

    const ParentSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      child: foreign(target),
      children: relation(target, {
        direction: "outgoing",
        relation: "children",
        cardinality: "many",
      }),
    })
    const Parent = Model.define("parent", ParentSchema)

    expect(targetCalls).toBe(0)
    expect(Parent.idField).toBe("id")
    expect(Parent.fields.map((entry) => entry.path.join("."))).toEqual(["id", "child", "children"])
    expect(Parent.fields[1]?.metadata.foreign?.target).toBe(target)
    expect(Parent.fields[2]?.metadata.relate?.target).toBe(target)
  })

  it("does not promote inline child metadata into the parent root", () => {
    const ChildSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String,
    })
    const Parent = Model.define(
      "parent-inline",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        inline: ChildSchema,
      }),
    )

    expect(Parent.idField).toBe("id")
    expect(Parent.uniqueFields).toEqual([])
    expect(Parent.paginationField).toBeUndefined()
    expect(Parent.fieldNames).toEqual(["id", "inline"])
  })

  it("defines SQL views with typed params and an explicit result slot", () => {
    const Params = Schema.Struct({
      min: Schema.Int,
      table: Schema.String,
    })
    const Rows = Schema.Struct({
      id: RootIdSchema,
      label: Schema.String,
    })
    const SqlView = Model.sqlView("labels", Rows, {
      params: Params,
      sql: "SELECT record::id(id) AS id, label FROM $table WHERE score >= $min;",
      bind: (statement, params) => {
        const min: number = params.min
        const table: string = params.table
        void [min, table]
        return statement
      },
      resultIndex: 2,
    })

    expect(SqlView.source.kind).toBe("sql")
    if (SqlView.source.kind !== "sql") throw new Error("expected a SQL view model")
    expect(SqlView.source.name).toBe("labels")
    expect(SqlView.source.query.params).toBe(Params)
    expect(SqlView.source.query.sql).toContain("SELECT")
    expect(SqlView.source.query.resultIndex).toBe(2)
    expect(SqlView.source.query.bind).toBeTypeOf("function")
    expect(() => Model.sqlView("bad", Rows, { params: Params, sql: "RETURN NONE;", resultIndex: -1 })).toThrowError(
      DbError,
    )
  })

  it("carries a distinct foreign target codec service into the parent schema", () => {
    class ForeignCodecEnv extends Context.Service<ForeignCodecEnv, { readonly prefix: string }>()(
      "appdb/tests/ForeignCodecEnv",
    ) {}

    const TargetSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      label: Schema.String.pipe(
        Schema.decodeTo(Schema.String, {
          decode: SchemaGetter.transformEffect<string, string, ForeignCodecEnv>((value) =>
            ForeignCodecEnv.use((env) => Effect.succeed(`${env.prefix}${value}`)),
          ),
          encode: SchemaGetter.transformEffect<string, string, ForeignCodecEnv>((value) =>
            ForeignCodecEnv.use((env) => Effect.succeed(`${env.prefix}${value}`)),
          ),
        }),
      ),
    })
    const Target = Model.define("target-service", TargetSchema)
    const Parent = Model.define(
      "parent-service",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Schema.Struct({ id: RootIdSchema }), {
          foreign: { target: () => Target },
        }),
      }),
    )

    type Includes<Whole, Need> = [Need] extends [Whole] ? true : false
    const carriesDecodeService: Includes<
      typeof Parent.schema["DecodingServices"],
      ForeignCodecEnv
    > = true
    const carriesEncodeService: Includes<
      typeof Parent.schema["EncodingServices"],
      ForeignCodecEnv
    > = true
    expect(carriesDecodeService).toBe(true)
    expect(carriesEncodeService).toBe(true)
  })
})
