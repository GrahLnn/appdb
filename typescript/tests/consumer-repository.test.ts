import { Context, Effect, Layer, Schema, SchemaGetter } from "effect"
import { BoundQuery, RecordId, Table } from "surrealdb"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { GraphRepo } from "../src/graph.js"
import { RootIdSchema, makeRecordId } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { PaginationPlan, queryPage } from "../src/pagination.js"
import { appendOrderedRelationEdges } from "../src/relation.js"
import { hydrateRow, makeStore, makeView, type RepositoryError } from "../src/repository.js"
import { queryTake, rawSql } from "../src/query.js"
import { field } from "../src/schema.js"

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "consumer-repository",
          database: "consumer-repository",
        }),
      ),
    ),
  )

describe("repository consumers", () => {
  it("round-trips nullable, optional, and array foreign fields through the store", async () => {
    const Child = Model.define(
      "repository_nested_child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const foreign = { foreign: { target: () => Child } } as const
    const Parent = Model.define(
      "repository_nested_parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        nullableChild: field(Schema.NullOr(Child.schema), foreign),
        optionalChild: field(Schema.optionalKey(Child.schema), foreign),
        children: field(Schema.Array(Child.schema), foreign),
      }),
    )
    const store = makeStore(Parent)
    const input = {
      id: "parent-1",
      nullableChild: { id: "child-1", label: "one" },
      children: [
        { id: "child-2", label: "two" },
        { id: "child-3", label: "three" },
      ],
    }
    const secondInput = {
      id: "parent-2",
      nullableChild: null,
      optionalChild: { id: "child-1", label: "one" },
      children: [{ id: "child-3", label: "three" }],
    }

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const saved = yield* store.save(input)
        const savedSecond = yield* store.save(secondInput)
        const fetched = yield* store.get("parent-1")
        const fetchedSecond = yield* store.get("parent-2")
        const database = yield* Database
        const slots = yield* database.query(
          "SELECT record::id(id) AS id, nullableChild, optionalChild, children FROM repository_nested_parent ORDER BY id ASC;",
        )
        return { saved, savedSecond, fetched, fetchedSecond, raw: slots[0] }
      }),
    )

    expect(result.saved).toEqual(input)
    expect(result.savedSecond).toEqual(secondInput)
    expect(result.fetched).toEqual(input)
    expect(result.fetchedSecond).toEqual(secondInput)
    const raw = result.raw as readonly {
      id: string
      nullableChild: unknown
      optionalChild?: unknown
      children: unknown
    }[]
    expect(raw).toHaveLength(2)
    expect(raw[0]?.id).toBe("parent-1")
    expect(raw[0]?.nullableChild).toBeInstanceOf(RecordId)
    expect(raw[0]?.optionalChild).toBeUndefined()
    expect(raw[0]?.children).toEqual([
      new RecordId("repository_nested_child", "child-2"),
      new RecordId("repository_nested_child", "child-3"),
    ])
    expect(raw[1]?.id).toBe("parent-2")
    expect(raw[1]?.nullableChild).toBeNull()
    expect(raw[1]?.optionalChild).toBeInstanceOf(RecordId)
  })

  it("preflights duplicate saveMany identities before parent or child writes", async () => {
    const Child = Model.define(
      "repository_duplicate_child",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const Parent = Model.define(
      "repository_duplicate_parent",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        child: field(Child.schema, { foreign: { target: () => Child } }),
      }),
    )
    const store = makeStore(Parent)
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE repository_duplicate_child:existing CONTENT { label: 'existing' }; CREATE repository_duplicate_parent:existing CONTENT { child: repository_duplicate_child:existing };",
        )
        const outcome = yield* Effect.exit(
          store.saveMany([
            { id: "duplicate", child: { id: "child-a", label: "A" } },
            { id: "duplicate", child: { id: "child-b", label: "B" } },
          ]),
        )
        const parents = yield* database.query(
          "SELECT record::id(id) AS id FROM repository_duplicate_parent ORDER BY id ASC;",
        )
        const children = yield* database.query(
          "SELECT record::id(id) AS id FROM repository_duplicate_child ORDER BY id ASC;",
        )
        return { outcome, parents, children }
      }),
    )

    expect(result.outcome._tag).toBe("Failure")
    expect(result.parents[0]).toEqual([{ id: "existing" }])
    expect(result.children[0]).toEqual([{ id: "existing" }])
  })

  it("keeps graph direction and ordered relation positions observable", async () => {
    const root = makeRecordId("repository_relation_node", "root")
    const first = makeRecordId("repository_relation_node", "first")
    const second = makeRecordId("repository_relation_node", "second")
    const third = makeRecordId("repository_relation_node", "third")
    const relation = "repository_relation_edges"
    const orderedRelation = "repository_ordered_edges"

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE repository_relation_node:root CONTENT { label: 'root' }; CREATE repository_relation_node:first CONTENT { label: 'first' }; CREATE repository_relation_node:second CONTENT { label: 'second' }; CREATE repository_relation_node:third CONTENT { label: 'third' };",
        )
        yield* GraphRepo.relateAt(root, first, relation)
        yield* GraphRepo.backRelateAt(root, second, relation)
        const outgoing = yield* GraphRepo.outgoingIds(root, relation)
        const incoming = yield* GraphRepo.incomingIds(root, relation)

        const ordered = appendOrderedRelationEdges(
          new BoundQuery("RETURN NONE;"),
          orderedRelation,
          [
            { in: root, out: second, position: 2 },
            { in: root, out: first, position: 1 },
            { in: root, out: third, position: 3 },
          ],
          "ordered",
        )
        yield* database.query(ordered.statement)
        const edges = yield* GraphRepo.outgoingEdges(root, orderedRelation)
        return { outgoing, incoming, edges }
      }),
    )

    expect(result.outgoing).toEqual([first])
    expect(result.incoming).toEqual([second])
    expect(result.edges.map((edge) => [edge.out, edge.position])).toEqual([
      [first, 1],
      [second, 2],
      [third, 3],
    ])
  })

  it("synchronizes ordered scalar, optional, and array relation cardinalities", async () => {
    const Target = Model.define(
      "repository_cardinality_target",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const relation = (name: string) => ({
      relate: {
        target: () => Target,
        direction: "outgoing" as const,
        relation: name,
      },
    })
    const Scalar = Model.define(
      "repository_scalar_relation",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        target: field(Target.schema, relation("repository_scalar_edges")),
      }),
    )
    const Optional = Model.define(
      "repository_optional_relation",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        target: Schema.optionalKey(field(Target.schema, relation("repository_optional_edges"))),
      }),
    )
    const Collection = Model.define(
      "repository_array_relation",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        targets: field(Schema.Array(Target.schema), relation("repository_array_edges")),
      }),
    )

    expect(Scalar.fields.find((item) => item.path.join(".") === "target")?.metadata.relate).toMatchObject({
      direction: "outgoing",
      relation: "repository_scalar_edges",
      target: expect.any(Function),
    })
    expect(Optional.fields.find((item) => item.path.join(".") === "target")?.metadata.relate).toMatchObject({
      direction: "outgoing",
      relation: "repository_optional_edges",
      target: expect.any(Function),
    })
    expect(Scalar.recordId("one").table.name).toBe("repository_scalar_relation")
    expect(Optional.recordId("one").table.name).toBe("repository_optional_relation")

    const scalarStore = makeStore(Scalar)
    const optionalStore = makeStore(Optional)
    const collectionStore = makeStore(Collection)
    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE repository_cardinality_target:one CONTENT { label: 'one' }; CREATE repository_cardinality_target:two CONTENT { label: 'two' }; CREATE repository_cardinality_target:three CONTENT { label: 'three' };",
        )
        const scalar = yield* scalarStore.save({
          id: "scalar-1",
          target: { id: "one", label: "one" },
        })
        const optional = yield* optionalStore.save({ id: "optional-1" })
        const collection = yield* collectionStore.save({
          id: "array-1",
          targets: [
            { id: "two", label: "two" },
            { id: "one", label: "one" },
            { id: "three", label: "three" },
          ],
        })
        const scalarEdges = yield* GraphRepo.outgoingEdges(
          Scalar.recordId("scalar-1"),
          "repository_scalar_edges",
        )
        const arrayEdges = yield* GraphRepo.outgoingEdges(
          Collection.recordId("array-1"),
          "repository_array_edges",
        )
        return { scalar, optional, collection, scalarEdges, arrayEdges }
      }),
    )

    expect(result.scalar).toEqual({ id: "scalar-1", target: { id: "one", label: "one" } })
    expect(result.optional).toEqual({ id: "optional-1" })
    expect(result.collection).toEqual({
      id: "array-1",
      targets: [
        { id: "two", label: "two" },
        { id: "one", label: "one" },
        { id: "three", label: "three" },
      ],
    })
    expect(result.scalarEdges.map((edge) => [edge.out.id, edge.position])).toEqual([["one", 0]])
    expect(result.arrayEdges.map((edge) => [edge.out.id, edge.position])).toEqual([
      ["two", 0],
      ["one", 1],
      ["three", 2],
    ])
  })

  it("reads SQL view rows through a bound query and reports the makeView boundary", async () => {
    const Owner = Model.define(
      "repository_sql_view_owner",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
    )
    const SqlView = Model.view(
      Owner,
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        label: Schema.String,
      }),
      { readSource: "sql" },
    )
    const view = makeView(SqlView)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query("CREATE repository_sql_view_owner:one CONTENT { label: 'visible' };")
        const rows = yield* queryTake(
          rawSql(
            "RETURN NONE; SELECT record::id(id) AS id, label FROM $table WHERE label = $label ORDER BY id ASC;",
            { table: new Table(Owner.table), label: "visible" },
          ),
          SqlView.schema,
          1,
        )
        const hydrated = yield* hydrateRow(SqlView, rows[0])
        const get = yield* Effect.exit(view.get("one"))
        const list = yield* Effect.exit(view.list())
        return { rows, hydrated, get, list }
      }),
    )

    expect(result.rows).toEqual([{ id: "one", label: "visible" }])
    expect(result.hydrated).toEqual({ id: "one", label: "visible" })
    expect(result.get._tag).toBe("Failure")
    expect(result.list._tag).toBe("Failure")
  })

  it("walks equal sort keys exactly once across queryPage cursors", async () => {
    const PageModel = Model.define(
      "repository_page_rows",
      Schema.Struct({
        id: field(RootIdSchema, { id: true }),
        rank: field(Schema.Int, { pagination: true }),
      }),
    )
    const plan = new PaginationPlan(PageModel, "rank", "asc")
    const descPlan = new PaginationPlan(PageModel, "rank", "desc")
    const cursorAfterEqualKey = plan.buildCursor({ id: "a2", rank: 1 })
    const cursorBeforeEqualKey = descPlan.buildCursor({ id: "b1", rank: 2 })

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const database = yield* Database
        yield* database.query(
          "CREATE repository_page_rows:a1 CONTENT { rank: 1 }; CREATE repository_page_rows:a2 CONTENT { rank: 1 }; CREATE repository_page_rows:b1 CONTENT { rank: 2 }; CREATE repository_page_rows:b2 CONTENT { rank: 2 };",
        )
        const first = yield* queryPage(PageModel, 2, undefined, "asc")
        const after = yield* queryPage(PageModel, 2, cursorAfterEqualKey, "asc")
        const before = yield* queryPage(PageModel, 2, cursorBeforeEqualKey, "desc")
        return { first, after, before }
      }),
    )

    const ids = (items: readonly unknown[]) =>
      items.map((item) => (item as { id: unknown }).id)
    expect(ids(result.first.items)).toEqual(["a1", "a2"])
    expect(ids(result.after.items)).toEqual(["b1", "b2"])
    expect(ids(result.before.items)).toEqual(["a2", "a1"])
    expect(result.first.next).toBeDefined()
    expect(result.after.next).toBeUndefined()
  })
})

interface CodecState {
  readonly prefix: string
  encodeCalls: number
  decodeCalls: number
}

class CodecEnv extends Context.Service<CodecEnv, CodecState>()("appdb/tests/RepositoryCodecEnv") {}

const repositoryCodec = Schema.String.pipe(
  Schema.decodeTo(Schema.Number, {
    decode: SchemaGetter.transformEffect<number, string, CodecEnv>((encoded) =>
      CodecEnv.use((env) => Effect.succeed(Number(encoded.slice(env.prefix.length)))),
    ),
    encode: SchemaGetter.transformEffect<string, number, CodecEnv>((decoded) =>
      CodecEnv.use((env) => Effect.succeed(`${env.prefix}${decoded}`)),
    ),
  }),
)

const RepositoryCodecChildSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  value: repositoryCodec,
})
const RepositoryCodecChild = Model.define("repository_codec_child", RepositoryCodecChildSchema)
const RepositoryCodecParentSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  child: field(RepositoryCodecChildSchema, { foreign: { target: () => RepositoryCodecChild } }),
})
const RepositoryCodecParent = Model.define("repository_codec_parent", RepositoryCodecParentSchema)
const repositoryCodecInput: typeof RepositoryCodecParentSchema["Type"] = {
  id: "parent-1",
  child: { id: "child-1", value: 7 },
}
const repositoryCodecEncodedInput: typeof RepositoryCodecParentSchema["Encoded"] = {
  id: "parent-1",
  child: { id: "child-1", value: "n:7" },
}
const repositoryCodecStore = makeStore(RepositoryCodecParent)

const assertRepositoryStoreEnvironment = (): void => {
  const save = repositoryCodecStore.save(repositoryCodecInput)
  const get = repositoryCodecStore.get("parent-1")
  const saveWithCodec: Effect.Effect<
    typeof RepositoryCodecParentSchema["Type"],
    RepositoryError,
    Database | CodecEnv
  > = save
  const getWithCodec: Effect.Effect<
    typeof RepositoryCodecParentSchema["Type"],
    RepositoryError,
    Database | CodecEnv
  > = get
  void saveWithCodec
  void getWithCodec
  const withCodec = Layer.succeed(CodecEnv, { prefix: "n:", encodeCalls: 0, decodeCalls: 0 })
  const saveAfterCodec: Effect.Effect<
    typeof RepositoryCodecParentSchema["Type"],
    RepositoryError,
    Database
  > = Effect.provide(save, withCodec)
  const getAfterCodec: Effect.Effect<
    typeof RepositoryCodecParentSchema["Type"],
    RepositoryError,
    Database
  > = Effect.provide(get, withCodec)
  void saveAfterCodec
  void getAfterCodec
  // @ts-expect-error A Database service is still required after providing CodecEnv.
  void Effect.runPromise(saveAfterCodec)
  // @ts-expect-error A Database service is still required after providing CodecEnv.
  void Effect.runPromise(getAfterCodec)
  // @ts-expect-error Store effects retain both Database and CodecEnv requirements.
  void Effect.runPromise(save)
  // @ts-expect-error Store effects retain both Database and CodecEnv requirements.
  void Effect.runPromise(get)
  const decodedNumber: number = repositoryCodecInput.child.value
  const encodedString: string = repositoryCodecEncodedInput.child.value
  // @ts-expect-error The decoded public value is a number, not its storage string.
  const decodedMustNotBeString: string = repositoryCodecInput.child.value
  // @ts-expect-error The encoded storage value is a string, not the decoded number.
  const encodedMustNotBeNumber: number = repositoryCodecEncodedInput.child.value
  void [decodedNumber, encodedString, decodedMustNotBeString, encodedMustNotBeNumber]
}

void assertRepositoryStoreEnvironment
