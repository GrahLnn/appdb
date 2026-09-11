import { Clock, Effect, Schema, SchemaGetter } from "effect"
import { Model, RootIdSchema, field, makeView } from "@appdb/core"
import { makeNodeDatabaseLayer, nodeEndpoint } from "@appdb/core/node"
import { Table } from "surrealdb"

const pendingOrResolved = Schema.Union([Schema.String, Schema.Undefined])
const FillNow = pendingOrResolved.pipe(
  Schema.decodeTo(pendingOrResolved, {
    decode: SchemaGetter.transformEffect<string | undefined, string | undefined>(
      (value) => Effect.succeed(value),
    ),
    encode: SchemaGetter.withDefault(
      Effect.map(Clock.currentTimeMillis, (milliseconds) => new Date(milliseconds).toISOString()),
    ),
  }),
)

const UserSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  email: field(Schema.String, { unique: true }),
  createdAt: FillNow,
})

const User = Model.define("user", UserSchema)

// Constructing the layer is lazy; this example does not start an engine or
// require a local database. It verifies the public Node entry point alongside
// the cross-platform core entry point.
const nodeLayer = makeNodeDatabaseLayer({
  endpoint: nodeEndpoint("mem"),
  namespace: "app",
  database: "app",
  schema: { models: [User] },
})

const LabelRowSchema = Schema.Struct({
  id: RootIdSchema,
  label: Schema.String,
})
const LabelParams = Schema.Struct({ minScore: Schema.Int })
const Labels = Model.sqlView("labels", LabelRowSchema, {
  params: LabelParams,
  sql: "RETURN NONE; SELECT record::id(id) AS id, label FROM user WHERE score >= $minScore;",
  resultIndex: 1,
})
const labelQuery = makeView(Labels).query({ minScore: 10 })

const DynamicLabelParams = Schema.Struct({ table: Schema.String, minScore: Schema.Int })
const DynamicLabels = Model.sqlView("dynamic_labels", LabelRowSchema, {
  params: DynamicLabelParams,
  sql: "RETURN NONE; SELECT record::id(id) AS id, label FROM $table WHERE score >= $minScore;",
  bind: (statement, encoded) =>
    statement.append("", {
      table: new Table(encoded.table),
      minScore: encoded.minScore,
    }),
  resultIndex: 1,
})
const dynamicLabelQuery = makeView(DynamicLabels).query({ table: "user", minScore: 10 })
void [labelQuery, dynamicLabelQuery]

const program = Effect.gen(function* () {
  const decoded = yield* User.decode({
    id: "alice",
    email: "alice@example.com",
    createdAt: undefined,
  })
  const encoded = yield* User.encode(decoded)

  return { decoded, encoded }
})

const result = await Effect.runPromise(program)

console.log({
  table: User.table,
  fields: User.fieldNames,
  decoded: result.decoded,
  encoded: result.encoded,
  nodeLayerDefined: nodeLayer !== undefined,
  sqlViewResultIndex: Labels.source.kind === "sql" ? Labels.source.query.resultIndex : undefined,
  customSqlViewResultIndex: DynamicLabels.source.kind === "sql" ? DynamicLabels.source.query.resultIndex : undefined,
})
