import { Effect, Schema } from "effect"
import { RecordId, type BoundQuery } from "surrealdb"
import { decodeError, DbError } from "./errors.js"
import { makeRecordId, type RootId } from "./id.js"
import {
  compileSchemaPlan,
  type CompiledSchemaPlan,
  type StorageFieldPlan,
} from "./schema.js"

export type SqlViewBinder<Params extends Schema.Top> = (
  statement: BoundQuery,
  params: Params["Encoded"],
) => BoundQuery

/**
 * A typed, static SQL projection. `params` is the native Effect Schema for
 * caller input; the repository owns execution and invokes `bind` after it has
 * encoded that input. The binder is the escape hatch for Surreal values such
 * as `Table`, `RecordId`, and `bigint` that cannot be represented by a plain
 * object spread.
 */
export interface SqlViewQuery<Params extends Schema.Top> {
  readonly params: Params
  readonly sql: string
  readonly bind?: SqlViewBinder<Params>
  readonly resultIndex: number
}

export type ModelSource<Table extends string, Params extends Schema.Top = Schema.Top> =
  | {
      readonly kind: "store"
      readonly table: Table
      readonly identity: symbol
      readonly ownerIdentity: symbol
    }
  | {
      readonly kind: "view"
      readonly table: Table
      readonly identity: symbol
      readonly ownerIdentity: symbol
      readonly owner: AnyModel
      readonly readSource: "table" | "sql"
    }
  | {
      /** SQL views have no table owner; `name` is only a logical model name. */
      readonly kind: "sql"
      readonly name: Table
      readonly identity: symbol
      /** Kept as a model identity for source-generic callers; no owner exists. */
      readonly ownerIdentity: symbol
      readonly query: SqlViewQuery<Params>
    }

export interface Model<
  Table extends string,
  C extends Schema.Top,
  Params extends Schema.Top = Schema.Top,
> {
  readonly table: Table
  readonly schema: C
  readonly source: ModelSource<Table, Params>
  readonly storage: CompiledSchemaPlan
  readonly fields: readonly StorageFieldPlan[]
  readonly fieldNames: readonly string[]
  readonly idField?: string
  readonly uniqueFields: readonly string[]
  readonly paginationField?: string
  readonly decode: (
    input: unknown,
  ) => Effect.Effect<C["Type"], DbError, C["DecodingServices"]>
  readonly encode: (
    value: C["Type"],
  ) => Effect.Effect<C["Encoded"], DbError, C["EncodingServices"]>
  readonly recordId: (id: RootId) => RecordId<Table, RootId>
}

// `any` is intentional for lazy owner/target references. The concrete model
// still carries its exact schema type at every public construction site.
export type AnyModel = Model<string, any, any>

export interface ViewOptions {
  readonly readSource?: "table" | "sql"
}

export interface SqlViewOptions<Params extends Schema.Top> {
  readonly params: Params
  readonly sql: string
  readonly bind?: SqlViewBinder<Params>
  readonly resultIndex?: number
}

export interface DefineOptions {
  readonly identity?: symbol
}

const decode = <C extends Schema.Top>(
  schema: C,
  input: unknown,
): Effect.Effect<C["Type"], DbError, C["DecodingServices"]> =>
  Schema.decodeUnknownEffect(schema)(input).pipe(Effect.mapError((cause) => decodeError(cause)))

const encode = <C extends Schema.Top>(
  schema: C,
  value: C["Type"],
): Effect.Effect<C["Encoded"], DbError, C["EncodingServices"]> =>
  Schema.encodeEffect(schema)(value).pipe(Effect.mapError((cause) => decodeError(cause)))

const makeModel = <Table extends string, C extends Schema.Top, Params extends Schema.Top>(
  table: Table,
  schema: C,
  source: ModelSource<Table, Params>,
): Model<Table, C, Params> => {
  const storage = compileSchemaPlan(schema)
  const model: {
    table: Table
    schema: C
    source: ModelSource<Table, Params>
    storage: CompiledSchemaPlan
    fields: readonly StorageFieldPlan[]
    fieldNames: readonly string[]
    idField?: string
    uniqueFields: readonly string[]
    paginationField?: string
    decode: (
      input: unknown,
    ) => Effect.Effect<C["Type"], DbError, C["DecodingServices"]>
    encode: (
      value: C["Type"],
    ) => Effect.Effect<C["Encoded"], DbError, C["EncodingServices"]>
    recordId: (id: RootId) => RecordId<Table, RootId>
  } = {
    table,
    schema,
    source,
    storage,
    fields: storage.fields,
    fieldNames: storage.fieldNames,
    uniqueFields: storage.uniqueFields,
    decode: (input) => decode(schema, input),
    encode: (value) => encode(schema, value),
    recordId: (id) => makeRecordId(table, id),
  }
  if (storage.idField !== undefined) {
    model.idField = storage.idField
  }
  if (storage.paginationField !== undefined) {
    model.paginationField = storage.paginationField
  }
  return model
}

export const define = <Table extends string, C extends Schema.Top>(
  table: Table,
  schema: C,
  options: DefineOptions = {},
): Model<Table, C> => {
  const identity = options.identity ?? Symbol(`appdb/store/${table}`)
  return makeModel<Table, C, Schema.Top>(table, schema, {
    kind: "store",
    table,
    identity,
    ownerIdentity: identity,
  })
}

export const view = <
  Table extends string,
  OwnerSchema extends Schema.Top,
  C extends Schema.Top,
>( 
  owner: Model<Table, OwnerSchema>,
  schema: C,
  options: ViewOptions = {},
): Model<Table, C> => {
  if (owner.source.kind === "sql") {
    throw new DbError({
      kind: "InvalidModel",
      message: "SQL views cannot be used as table-view owners",
      operation: "define view",
    })
  }
  const identity = Symbol(`appdb/view/${owner.table}`)
  return makeModel<Table, C, Schema.Top>(owner.table, schema, {
    kind: "view",
    table: owner.table,
    identity,
    ownerIdentity: owner.source.ownerIdentity,
    owner,
    readSource: options.readSource ?? "table",
  })
}

const validateResultIndex = (resultIndex: number): number => {
  if (!Number.isSafeInteger(resultIndex) || resultIndex < 0) {
    throw new DbError({
      kind: "InvalidModel",
      message: "SQL view resultIndex must be a non-negative safe integer",
      operation: "define SQL view",
    })
  }
  return resultIndex
}

/**
 * Defines a read-only SQL view without inventing a table owner. The logical
 * name is retained for model identity and diagnostics only; repository SQL
 * view execution uses the descriptor's statement and result slot.
 */
export const sqlView = <
  Name extends string,
  C extends Schema.Top,
  Params extends Schema.Top,
>(
  name: Name,
  schema: C,
  options: SqlViewOptions<Params>,
): Model<Name, C, Params> => {
  const resultIndex = validateResultIndex(options.resultIndex ?? 0)
  const identity = Symbol(`appdb/sql-view/${name}`)
  return makeModel<Name, C, Params>(name, schema, {
    kind: "sql",
    name,
    identity,
    ownerIdentity: identity,
    query: {
      params: options.params,
      sql: options.sql,
      ...(options.bind === undefined ? {} : { bind: options.bind }),
      resultIndex,
    },
  })
}

/** Public namespace-style API: `Model.define(...)` and `Model.view(...)`. */
export const Model = { define, view, sqlView }
