import { Effect, Option, Schema, SchemaAST } from "effect"
import { RecordId, Table, type Patch } from "surrealdb"

import {
  Crypto,
  CRYPTO_NONCE_BYTES,
  CRYPTO_TAG_BYTES,
  isByteContainer,
  makeCryptoError,
  normalizeBytes,
  type CryptoApi,
  type CryptoError,
  type KeyContext,
} from "./crypto.js"
import { Database } from "./connection.js"
import {
  decodeError,
  DbError,
  DbErrorKind,
  errorMessage,
  makeDbError,
} from "./errors.js"
import {
  assertRootId,
  isRecordId,
  isRootId,
  type RootId,
} from "./id.js"
import type { AnyModel, Model } from "./model.js"
import { query, rawSql, validateIdentifier } from "./query.js"
import { incomingRowsForOwnersStatement, outgoingRowsForOwnersStatement } from "./relation.js"
import { fieldSchemaOf, type FieldPath, type StorageFieldPlan } from "./schema.js"
import { WritePlanBuilder, type ResultSlot, type WriteMode } from "./write-plan.js"

export type RepositoryError = DbError | CryptoError

type ModelSchema<M extends AnyModel> = M["schema"]
type ModelValue<M extends AnyModel> = ModelSchema<M>["Type"]
type ViewModel = Model<string, any, any>
type ViewModelSchema<M extends ViewModel> = M["schema"]
type ViewModelValue<M extends ViewModel> = ViewModelSchema<M>["Type"]
type ModelParams<M extends ViewModel> = M extends Model<string, any, infer Params> ? Params : Schema.Top
type ModelServices<M extends AnyModel> =
  | Database
  | ModelSchema<M>["EncodingServices"]
  | ModelSchema<M>["DecodingServices"]
type ViewServices<M extends ViewModel> =
  | Database
  | ViewModelSchema<M>["DecodingServices"]
  | ModelParams<M>["EncodingServices"]
type ReturningServices<M extends AnyModel, V extends ViewModel> =
  | ModelServices<M>
  | ViewModelSchema<V>["EncodingServices"]
  | ViewModelSchema<V>["DecodingServices"]

export interface Store<M extends AnyModel> {
  readonly returning: <V extends ViewModel>(view: V) => ReturningStore<M, V>
  readonly create: (value: ModelValue<M>) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly createAt: (
    id: RootId | RecordId,
    value: ModelValue<M>,
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly upsertAt: (
    id: RootId | RecordId,
    value: ModelValue<M>,
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly updateAt: (
    id: RootId | RecordId,
    value: ModelValue<M>,
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly save: (value: ModelValue<M>) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly saveMany: (
    values: readonly ModelValue<M>[],
  ) => Effect.Effect<readonly ModelValue<M>[], RepositoryError, ModelServices<M>>
  readonly get: (
    id: RootId | RecordId,
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly getRecord: (
    id: RecordId,
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly list: () => Effect.Effect<readonly ModelValue<M>[], RepositoryError, ModelServices<M>>
  readonly exists: () => Effect.Effect<boolean, RepositoryError, ModelServices<M>>
  readonly delete: (id: RootId) => Effect.Effect<void, RepositoryError, ModelServices<M>>
  readonly deleteRecord: (id: RecordId) => Effect.Effect<void, RepositoryError, ModelServices<M>>
  readonly deleteAll: () => Effect.Effect<void, RepositoryError, ModelServices<M>>
  readonly listRecordIds: () => Effect.Effect<readonly RecordId[], RepositoryError, ModelServices<M>>
  readonly findOneId: (
    field: string,
    value: string,
  ) => Effect.Effect<RecordId, RepositoryError, ModelServices<M>>
  readonly merge: (
    id: RootId | RecordId,
    value: Record<string, unknown>,
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
  readonly patch: (
    id: RootId | RecordId,
    operations: readonly Patch[],
  ) => Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
}

/** Write operations that return an owner-preserving table View projection. */
export interface ReturningStore<M extends AnyModel, V extends ViewModel> {
  readonly create: (
    value: ModelValue<M>,
  ) => Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>
  readonly createAt: (
    id: RootId | RecordId,
    value: ModelValue<M>,
  ) => Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>
  readonly upsertAt: (
    id: RootId | RecordId,
    value: ModelValue<M>,
  ) => Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>
  readonly updateAt: (
    id: RootId | RecordId,
    value: ModelValue<M>,
  ) => Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>
  readonly save: (
    value: ModelValue<M>,
  ) => Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>
}

export interface ViewStore<M extends ViewModel> {
  /** Execute the model's typed SQL view query and hydrate every returned row. */
  readonly query: (
    params: ModelParams<M>["Type"],
  ) => Effect.Effect<readonly ViewModelValue<M>[], RepositoryError, ViewServices<M>>
  readonly get: (
    id: RootId | RecordId,
  ) => Effect.Effect<ViewModelValue<M>, RepositoryError, Database | ViewModelSchema<M>["DecodingServices"]>
  readonly list: () => Effect.Effect<readonly ViewModelValue<M>[], RepositoryError, Database | ViewModelSchema<M>["DecodingServices"]>
}

interface PlannedState {
  readonly builder: WritePlanBuilder
  readonly planned: Map<string, RecordId>
  readonly rawRows: Map<string, unknown>
  readonly hydrated: Map<string, unknown>
  /** Encoded schema values are the boundary passed into the owning model. */
  readonly hydratedEncoded: Map<string, unknown>
  readonly hydrating: Set<string>
  readonly encoding: Set<string>
  readonly planning: Set<string>
  readonly targets: WeakMap<object, AnyModel>
  batch?: HydrationBatch
}

interface PlanContext extends PlannedState {
  readonly rootModel: AnyModel
  readonly relationTables: Set<string>
}

interface HydrationBatch {
  readonly relationRows: Map<string, readonly unknown[]>
  readonly missingRows: Set<string>
}

interface HydrationNode {
  readonly model: AnyModel
  readonly record: RecordId
  readonly row: unknown
}

interface RelationBatchGroup {
  readonly model: AnyModel
  readonly plan: StorageFieldPlan
  readonly owners: Map<string, RecordId>
}

interface TargetBatchGroup {
  readonly model: AnyModel
  readonly records: Map<string, RecordId>
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value)

const own = (value: unknown, key: PropertyKey): boolean =>
  isObject(value) && Object.prototype.hasOwnProperty.call(value, key)

const dbFailure = <A>(error: DbError): Effect.Effect<A, DbError> => Effect.fail(error)

const asDbError = (cause: unknown, operation: string): DbError => {
  if (cause instanceof DbError) return cause
  return makeDbError(DbErrorKind.Decode, errorMessage(cause), { operation, cause })
}

const sync = <A>(operation: string, thunk: () => A): Effect.Effect<A, DbError> =>
  Effect.try({ try: thunk, catch: (cause) => asDbError(cause, operation) })

const modelTable = (model: AnyModel): string => validateIdentifier(model.table, "model table")

const recordKey = (record: RecordId): string => {
  const id = record.id
  const idText = typeof id === "object" ? JSON.stringify(id) : String(id)
  return `${record.table.name}\u0000${typeof id}\u0000${idText}`
}

let nextModelCacheId = 1
const modelCacheIds = new WeakMap<object, number>()

/** Keeps physical rows distinct across model/source/schema hydration owners. */
const modelCacheKey = (model: AnyModel): string => {
  const existing = modelCacheIds.get(model)
  if (existing !== undefined) return String(existing)
  const id = nextModelCacheId++
  modelCacheIds.set(model, id)
  return String(id)
}

const hydrationRowKey = (model: AnyModel, record: RecordId): string =>
  `${modelCacheKey(model)}\u0000${recordKey(record)}`

const rootRecord = (model: AnyModel, value: unknown): RecordId | undefined => {
  if (model.idField === undefined || !isObject(value)) return undefined
  const id = value[model.idField]
  if (isRecordId(id)) return id
  return isRootId(id) ? model.recordId(id) : undefined
}

const assertModelRecord = (model: AnyModel, record: RecordId, operation: string): RecordId => {
  if (record.table.name !== model.table) {
    throw new DbError({
      kind: DbErrorKind.InvalidIdentifier,
      message: `record id belongs to table ${record.table.name}, expected ${model.table}`,
      operation,
    })
  }
  return record
}

const generatedRecord = (model: AnyModel): RecordId =>
  model.recordId(`appdb-${globalThis.crypto.randomUUID()}`)

const pathValue = (value: unknown, path: FieldPath): unknown => {
  let current = value
  for (const part of path) {
    if (current === null || current === undefined) return undefined
    if (Array.isArray(current) && typeof part === "number") {
      current = current[part]
    } else if (isObject(current)) {
      current = current[String(part)]
    } else {
      return undefined
    }
  }
  return current
}

const copyContainer = (value: unknown): unknown => {
  if (Array.isArray(value)) return [...value]
  if (isObject(value)) return { ...value }
  return value
}

const putPath = (value: unknown, path: FieldPath, replacement: unknown): unknown => {
  if (path.length === 0) return replacement
  const [head, ...tail] = path
  const base = copyContainer(value)
  if (base === null || base === undefined || (typeof base !== "object" && !Array.isArray(base))) {
    return value
  }
  const container = base as Record<string, unknown> | unknown[]
  const key = typeof head === "number" ? head : String(head)
  const child = Array.isArray(container) ? container[key as number] : container[key as string]
  const next = putPath(child, tail, replacement)
  if (Array.isArray(container)) {
    container[key as number] = next
  } else {
    container[key as string] = next
  }
  return container
}

const removePath = (value: unknown, path: FieldPath): unknown => {
  if (path.length === 0) return value
  const [head, ...tail] = path
  const base = copyContainer(value)
  if (base === null || base === undefined || (typeof base !== "object" && !Array.isArray(base))) {
    return value
  }
  const container = base as Record<string, unknown> | unknown[]
  const key = typeof head === "number" ? head : String(head)
  if (tail.length === 0) {
    if (Array.isArray(container)) {
      delete container[key as number]
    } else {
      delete container[key as string]
    }
    return container
  }
  const child = Array.isArray(container) ? container[key as number] : container[key as string]
  const next = removePath(child, tail)
  if (Array.isArray(container)) {
    container[key as number] = next
  } else {
    container[key as string] = next
  }
  return container
}

const stripNullish = (value: unknown): unknown => {
  if (value instanceof RecordId || value instanceof Uint8Array || value instanceof Date) return value
  if (Array.isArray(value)) return value.map(stripNullish)
  if (!isObject(value)) return value
  const output: Record<string, unknown> = {}
  for (const [key, child] of Object.entries(value)) {
    // `undefined` represents an omitted optional field. Explicit `null` is a
    // caller value and must survive storage so nullable schemas can decode it.
    if (child === undefined) continue
    output[key] = stripNullish(child)
  }
  return output
}

const fieldPlans = (model: AnyModel, role: "foreign" | "relate" | "sensitive"): StorageFieldPlan[] =>
  model.fields.filter((field) => {
    if (role === "foreign") return field.metadata.foreign !== undefined
    if (role === "relate") return field.metadata.relate !== undefined
    return field.metadata.sensitive !== undefined
  })

const pathKey = (path: FieldPath): string => path.map((part) => String(part)).join("\u0000")

const targetForPlan = (plan: StorageFieldPlan, state: PlannedState): AnyModel | undefined => {
  const cached = state.targets.get(plan)
  if (cached !== undefined) return cached
  const target = plan.metadata.foreign?.target() ?? plan.metadata.relate?.target()
  if (target !== undefined) state.targets.set(plan, target)
  return target
}

const relationBatchKey = (
  model: AnyModel,
  owner: RecordId,
  plan: StorageFieldPlan,
): string => `${modelCacheKey(model)}\u0000${recordKey(owner)}\u0000${pathKey(plan.path)}`

const stripNullishAst = (ast: SchemaAST.AST): SchemaAST.AST => {
  if (!SchemaAST.isUnion(ast)) return ast
  const members = ast.types.filter((member) => member._tag !== "Null" && member._tag !== "Undefined")
  return members.length === 1 ? stripNullishAst(members[0]!) : ast
}

const relationIsArray = (plan: StorageFieldPlan): boolean =>
  SchemaAST.isArrays(stripNullishAst(plan.encodedAst))

const relationAllowsNull = (plan: StorageFieldPlan): boolean => {
  const ast = plan.encodedAst
  return SchemaAST.isUnion(ast) && ast.types.some((member) => member._tag === "Null")
}

const astAllowsUndefined = (ast: SchemaAST.AST): boolean =>
  (SchemaAST.isUnion(ast) && ast.types.some((member) => member._tag === "Undefined")) ||
    (ast as { readonly context?: { readonly isOptional?: boolean } }).context?.isOptional === true

const relationAllowsUndefined = (plan: StorageFieldPlan): boolean => {
  const ast = plan.encodedAst
  return astAllowsUndefined(ast)
}

/**
 * Returns whether a foreign field's own schema is the target schema, allowing
 * only the cardinality wrappers that the foreign writer traverses. Descending
 * through object properties would mistake `{ child: Target }` for a Target
 * row and would let the wrapper be written into the target table.
 */
const directlyCarriesSchema = (field: SchemaAST.AST, target: SchemaAST.AST): boolean => {
  if (field === target) return true
  if (SchemaAST.isSuspend(field)) return directlyCarriesSchema(field.thunk(), target)
  if (SchemaAST.isUnion(field)) {
    const members = field.types.filter((member) => member._tag !== "Null" && member._tag !== "Undefined")
    return members.length === 1 && directlyCarriesSchema(members[0]!, target)
  }
  if (SchemaAST.isArrays(field)) {
    const members = [...field.elements, ...field.rest]
    return members.length === 1 && directlyCarriesSchema(members[0]!, target)
  }
  return false
}

const fieldCarriesTargetSchema = (fieldSchema: Schema.Top | undefined, target: AnyModel): boolean => {
  if (fieldSchema === undefined) return false
  return directlyCarriesSchema(fieldSchema.ast, target.schema.ast) ||
    directlyCarriesSchema(SchemaAST.toEncoded(fieldSchema.ast), SchemaAST.toEncoded(target.schema.ast))
}

interface PlannedRelation {
  readonly plan: StorageFieldPlan
  readonly owner: RecordId
  readonly targets: readonly RecordId[]
}

interface EncodedStorage {
  readonly value: unknown
  readonly relationEncoded: ReadonlyMap<string, unknown>
}

const rawSlot = (slot: unknown): unknown => {
  if (slot === undefined || slot === null) return undefined
  if (Array.isArray(slot)) return slot.length === 0 ? undefined : slot[0]
  return slot
}

const rawRows = (slot: unknown): readonly unknown[] => {
  if (slot === undefined || slot === null) return []
  return Array.isArray(slot) ? slot : [slot]
}

const rowObject = (row: unknown, operation: string): Record<string, unknown> => {
  if (!isObject(row)) {
    throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} returned a non-object row`, operation })
  }
  return row
}

const stableRowValue = (value: unknown): string => {
  try {
    const encoded = JSON.stringify(value, (_key, item: unknown) => {
      if (typeof item === "bigint") return `${item}n`
      if (isRecordId(item)) return item.toString()
      return item
    })
    return encoded === undefined ? String(value) : encoded
  } catch {
    return String(value)
  }
}

const recordFromRaw = (model: AnyModel, value: unknown, operation: string): RecordId => {
  if (isRecordId(value)) {
    // A SQL view has no table owner. Its query may nevertheless return a real
    // RecordId from an arbitrary source table; retain that identity exactly.
    return model.source.kind === "sql" ? value : assertModelRecord(model, value, operation)
  }
  if (isRootId(value)) return model.recordId(value)
  if (typeof value === "string" && value.includes(":")) {
    const separator = value.indexOf(":")
    const table = value.slice(0, separator)
    const key = value.slice(separator + 1).replace(/^`|`$/g, "")
    if (table === model.table && key.length > 0) {
      const parsed = /^-?\d+$/.test(key) ? BigInt(key) : key
      return model.recordId(assertRootId(parsed, operation))
    }
  }
  throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} did not contain a valid RecordId`, operation })
}

const rowRecord = (model: AnyModel, row: Record<string, unknown>, operation: string): RecordId | undefined => {
  const raw = row.id ?? (model.idField === undefined ? undefined : row[model.idField])
  if (model.source.kind === "sql") {
    if (isRecordId(raw)) return raw
    // A projected SQL id is usually a scalar key (record::id(id)). Keep that
    // key in the decoded row without inventing the logical view name as a
    // RecordId table. Foreign/relation paths still require a real RecordId.
    if (isRootId(raw)) return undefined
  }
  return recordFromRaw(model, raw, operation)
}

const publicRow = (model: AnyModel, row: Record<string, unknown>, record: RecordId | undefined): Record<string, unknown> => {
  const output = { ...row }
  if (model.source.kind === "sql") {
    if (model.idField !== undefined) {
      if (record !== undefined) output[model.idField] = assertRootId(record.id, "decode record id key")
      if (model.idField !== "id") delete output.id
    } else if (record !== undefined) {
      output.id = assertRootId(record.id, "decode record id key")
    }
    return output
  }
  if (record === undefined) {
    throw new DbError({ kind: DbErrorKind.Decode, message: "table row did not contain a valid RecordId", operation: "decode record id" })
  }
  if (model.idField !== undefined) {
    output[model.idField] = assertRootId(record.id, "decode record id key")
    if (model.idField !== "id") delete output.id
  } else {
    delete output.id
  }
  return output
}

const rowKey = (model: AnyModel, row: Record<string, unknown>, record: RecordId | undefined): string => {
  if (record !== undefined) return recordKey(record)
  if (model.source.kind !== "sql") {
    throw new DbError({ kind: DbErrorKind.Decode, message: "table row did not contain a valid RecordId", operation: "row identity" })
  }
  const raw = model.idField === undefined ? row.id : row[model.idField] ?? row.id
  return `sql\u0000${String(model.source.identity)}\u0000${stableRowValue(raw ?? row)}`
}

const stableIdentity = (model: AnyModel, value: unknown): string | undefined => {
  const explicit = rootRecord(model, value)
  if (explicit !== undefined) return recordKey(explicit)
  // saveMany's preflight preserves its existing batch contract: only an
  // explicit record id or declared unique fields identify the same input.
  // The broader lookupFields fallback belongs to single-value save/foreign
  // resolution and must not turn identical id-less payloads into a batch
  // conflict.
  const fields = model.uniqueFields
  if (fields.length === 0 || !isObject(value)) return undefined
  const parts: string[] = []
  for (const field of fields) {
    if (!own(value, field) || value[field] === undefined || value[field] === null) return undefined
    const current = value[field]
    if (isObject(current) || Array.isArray(current)) {
      parts.push(JSON.stringify(current, (_key, item: unknown) => typeof item === "bigint" ? `${item}n` : item))
    } else {
      parts.push(`${typeof current}:${String(current)}`)
    }
  }
  return `${model.table}\u0000unique\u0000${parts.join("\u0000")}`
}

const uniqueFieldPlan = (model: AnyModel, field: string): StorageFieldPlan | undefined =>
  model.fields.find((entry) => entry.path.length === 1 && entry.path[0] === field)

/**
 * Rust's derived lookup metadata uses explicit unique fields when present;
 * otherwise it falls back to every top-level field that is safe to query.
 * Nested fields are storage details, while sensitive and relation-backed
 * fields are intentionally excluded from automatic identity resolution.
 */
const lookupFields = (model: AnyModel): readonly string[] => {
  if (model.uniqueFields.length > 0) return model.uniqueFields
  return model.fieldNames.filter((field) => {
    if (field === "id" || field === model.idField) return false
    const plan = uniqueFieldPlan(model, field)
    return plan?.metadata.sensitive === undefined && plan?.metadata.relate === undefined
  })
}

const resolveLookupForeign = (
  model: AnyModel,
  value: unknown,
  useFallback: boolean,
  targets?: WeakMap<object, AnyModel>,
): Effect.Effect<unknown, RepositoryError, Database> =>
  Effect.gen(function* () {
    if (value === null || value === undefined) return undefined
    if (Array.isArray(value)) {
      const resolved: RecordId[] = []
      for (const item of value) {
        const next = yield* resolveLookupForeign(model, item, useFallback, targets)
        if (next === undefined) return undefined
        if (Array.isArray(next)) resolved.push(...next)
        else if (isRecordId(next)) resolved.push(next)
        else return undefined
      }
      return resolved
    }
    if (isRecordId(value)) return assertModelRecord(model, value, "lookup foreign")
    return yield* lookupRecord(model, value, useFallback, targets)
  })

const lookupRecord = (
  model: AnyModel,
  value: unknown,
  useFallback = true,
  targets?: WeakMap<object, AnyModel>,
): Effect.Effect<RecordId | undefined, RepositoryError, Database> =>
  Effect.gen(function* () {
    if (isRecordId(value)) return assertModelRecord(model, value, "lookup")
    const explicit = rootRecord(model, value)
    if (explicit !== undefined) return assertModelRecord(model, explicit, "lookup")
    if (!isObject(value)) return undefined
    const fields = useFallback ? lookupFields(model) : model.uniqueFields
    if (fields.length === 0) return undefined
    const bindings: Record<string, unknown> = { table: new Table(modelTable(model)) }
    const predicates: string[] = []
    for (const [index, field] of fields.entries()) {
      const fieldValue = value[field]
      if (fieldValue === undefined || fieldValue === null) return undefined
      const plan = uniqueFieldPlan(model, field)
      let boundValue: unknown = fieldValue
      if (plan?.metadata.foreign !== undefined) {
        const cached = targets?.get(plan)
        const target = cached ?? plan.metadata.foreign.target()
        if (target !== undefined) targets?.set(plan, target)
        if (target === undefined) return undefined
        const resolved = yield* resolveLookupForeign(target, fieldValue, useFallback, targets)
        if (resolved === undefined) return undefined
        boundValue = resolved
      }
      const binding = `unique_${index}`
      bindings[binding] = boundValue
      validateIdentifier(field, "unique field")
      predicates.push(`${field} = $${binding}`)
    }

    const slots = yield* query(rawSql(
      `SELECT record::id(id) AS id FROM $table WHERE ${predicates.join(" AND ")} LIMIT 2;`,
      bindings,
    ))
    const rows = rawRows(slots[0])
    if (rows.length === 0) return undefined
    if (rows.length > 1) {
      return yield* dbFailure<RecordId | undefined>(new DbError({
        kind: DbErrorKind.InvalidModel,
        message: `unique lookup for ${model.table} matched multiple records`,
        operation: "lookup",
      }))
    }
    return recordFromRaw(model, rowObject(rows[0], "lookup" ).id, "lookup")
  })

const modelRecord = (
  model: AnyModel,
  value: unknown,
  allowLookup: boolean,
  useFallback = true,
  targets?: WeakMap<object, AnyModel>,
): Effect.Effect<RecordId, RepositoryError, Database> =>
  Effect.gen(function* () {
    const explicit = rootRecord(model, value)
    if (explicit !== undefined) return assertModelRecord(model, explicit, "model identity")
    const fields = useFallback ? lookupFields(model) : model.uniqueFields
    if (allowLookup && fields.length > 0) {
      const found = yield* lookupRecord(model, value, useFallback, targets)
      if (found !== undefined) return found
    }
    return generatedRecord(model)
  })

const secureContext = (model: AnyModel, plan: StorageFieldPlan): KeyContext => {
  const supplied = plan.metadata.sensitive?.keyContext
  return supplied ?? { model: model.table, field: plan.path.join(".") }
}

const storedBytes = (value: unknown, context: KeyContext): Effect.Effect<Uint8Array, CryptoError> =>
  Effect.try({
    try: () => normalizeBytes(value),
    catch: (cause) => makeCryptoError("decrypt", "stored ciphertext must be a byte container", context, cause),
  })

// A plain Array is also the container for a sensitive leaf collection. Treat
// its numeric form as ciphertext only when it can contain the Rust wire
// envelope; empty and short arrays must remain ordinary collection values.
const isStoredCipherBytes = (value: unknown): boolean =>
  isByteContainer(value) &&
    (!Array.isArray(value) || value.length >= CRYPTO_NONCE_BYTES + CRYPTO_TAG_BYTES)

const secureLeaves = (
  value: unknown,
  crypto: CryptoApi,
  context: KeyContext,
  encrypt: boolean,
): Effect.Effect<unknown, CryptoError> => {
  if (value === null || value === undefined) return Effect.succeed(value)
  if (!encrypt && isStoredCipherBytes(value)) {
    return storedBytes(value, context).pipe(Effect.flatMap((bytes) => crypto.decryptValue(bytes, context)))
  }
  if (encrypt && (Array.isArray(value) || isObject(value))) {
    if (Array.isArray(value)) {
      return Effect.forEach(value, (item) => secureLeaves(item, crypto, context, true), { concurrency: 1 })
    }
    return Effect.forEach(Object.entries(value), ([key, item]) =>
      secureLeaves(item, crypto, context, true).pipe(Effect.map((next) => [key, next] as const)),
      { concurrency: 1 },
    ).pipe(Effect.map((entries) => Object.fromEntries(entries)))
  }
  if (!encrypt && (Array.isArray(value) || isObject(value))) {
    if (Array.isArray(value)) {
      return Effect.forEach(value, (item) => secureLeaves(item, crypto, context, false), { concurrency: 1 })
    }
    return Effect.forEach(Object.entries(value), ([key, item]) =>
      secureLeaves(item, crypto, context, false).pipe(Effect.map((next) => [key, next] as const)),
      { concurrency: 1 },
    ).pipe(Effect.map((entries) => Object.fromEntries(entries)))
  }
  return encrypt
    ? crypto.encryptValue(value, context)
    : Effect.succeed(value)
}

const secureValue = (
  model: AnyModel,
  value: unknown,
  encrypt: boolean,
): Effect.Effect<unknown, RepositoryError> =>
  Effect.gen(function* () {
    let output = value
    const plans = fieldPlans(model, "sensitive")
    if (plans.length === 0) return output
    const cryptoOption = yield* Effect.serviceOption(Crypto)
    if (Option.isNone(cryptoOption)) {
      return yield* dbFailure<unknown>(new DbError({
        kind: DbErrorKind.NotInitialized,
        message: `sensitive model ${model.table} requires a Crypto service`,
        operation: encrypt ? "encode" : "decode",
      }))
    }
    for (const plan of plans) {
      const current = pathValue(output, plan.path)
      if (current === undefined || current === null) continue
      const scope = plan.metadata.sensitive?.scope ?? "value"
      const next = scope === "leaf"
        ? yield* secureLeaves(current, cryptoOption.value, secureContext(model, plan), encrypt)
        : encrypt
          ? yield* cryptoOption.value.encryptValue(current, secureContext(model, plan))
          : yield* storedBytes(current, secureContext(model, plan)).pipe(
            Effect.flatMap((bytes) => cryptoOption.value.decryptValue(bytes, secureContext(model, plan))),
          )
      output = putPath(output, plan.path, next)
    }
    return output
  })

const foreignRecord = (
  model: AnyModel,
  value: unknown,
  state: PlanContext,
  encodedValue?: unknown,
): Effect.Effect<unknown, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    if (value === null || value === undefined) return value
    if (Array.isArray(value)) {
      return yield* Effect.forEach(value, (item, index) =>
        foreignRecord(
          model,
          item,
          state,
          Array.isArray(encodedValue) ? encodedValue[index] : undefined,
        ),
        { concurrency: 1 },
      )
    }
    if (isRecordId(value)) return assertModelRecord(model, value, "foreign record")
    if (!isObject(value)) {
      return yield* dbFailure<unknown>(new DbError({
        kind: DbErrorKind.InvalidModel,
        message: `foreign field for ${model.table} must contain a model value or RecordId`,
        operation: "foreign write",
      }))
    }
    const record = yield* modelRecord(model, value, true, true, state.targets)
    // `encodedValue` is present only when the parent field was authored with
    // this exact target schema. In that case the parent encode already ran the
    // target codec, so running target.encode again would double-transform it.
    yield* planModelWrite(model, value, record, "upsertAt", state, "child", undefined, encodedValue)
    return record
  })

const relationRecords = (
  model: AnyModel,
  value: unknown,
  state: PlanContext,
  encodedValue?: unknown,
): Effect.Effect<readonly RecordId[], RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    if (value === null || value === undefined) return []
    if (Array.isArray(value)) {
      const output: RecordId[] = []
      for (const [index, item] of value.entries()) {
        const encodedItem = Array.isArray(encodedValue) ? encodedValue[index] : undefined
        output.push(...yield* relationRecords(model, item, state, encodedItem))
      }
      return output
    }
    if (isRecordId(value)) return [assertModelRecord(model, value, "relate record")]
    if (!isObject(value)) {
      return yield* dbFailure<readonly RecordId[]>(new DbError({
        kind: DbErrorKind.InvalidModel,
        message: `relate field for ${model.table} must contain a model value or RecordId`,
        operation: "relation write",
      }))
    }
    const record = yield* modelRecord(model, value, true, true, state.targets)
    yield* planModelWrite(model, value, record, "upsertAt", state, "child", undefined, encodedValue)
    return [record]
  })

const prepareRelationWrites = (
  model: AnyModel,
  value: unknown,
  owner: RecordId,
  state: PlanContext,
  relationEncoded: ReadonlyMap<string, unknown>,
): Effect.Effect<readonly PlannedRelation[], RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    const output: PlannedRelation[] = []
    for (const plan of fieldPlans(model, "relate")) {
      const metadata = plan.metadata.relate
      if (metadata === undefined) continue
      validateIdentifier(metadata.relation, "relation name")
      const target = targetForPlan(plan, state)
      if (target === undefined) continue
      const current = pathValue(value, plan.path)
      const encodedCurrent = fieldCarriesTargetSchema(fieldSchemaOf(plan.metadata), target)
        ? relationEncoded.get(pathKey(plan.path))
        : undefined
      const targets = yield* relationRecords(target, current, state, encodedCurrent)
      output.push({ plan, owner, targets })
    }
    return output
  })

const appendRelationWrites = (
  state: PlanContext,
  writes: readonly PlannedRelation[],
  prefix: string,
): void => {
  for (const [index, write] of writes.entries()) {
    const metadata = write.plan.metadata.relate
    if (metadata === undefined) continue
    const deleteOwnerBinding = `${prefix}_delete_owner_${index}`
    const anchor = metadata.direction === "outgoing" ? "in" : "out"
    // Relation tables are created lazily by SurrealDB. Bootstrap each table
    // once per write plan before synchronizing its first owner.
    if (!state.relationTables.has(metadata.relation)) {
      state.builder.appendStatement(
        `DEFINE TABLE IF NOT EXISTS ${metadata.relation} TYPE RELATION SCHEMALESS;`,
      )
      state.relationTables.add(metadata.relation)
    }
    state.builder.appendStatement(
      `DELETE FROM ${metadata.relation} WHERE ${anchor} = $${deleteOwnerBinding} RETURN NONE;`,
      {
        [deleteOwnerBinding]: write.owner,
      },
    )
    for (const [position, target] of write.targets.entries()) {
      const inBinding = `${prefix}_in_${index}_${position}`
      const outBinding = `${prefix}_out_${index}_${position}`
      const positionBinding = `${prefix}_position_${index}_${position}`
      state.builder.appendStatement(
        `RELATE $${inBinding} -> ${metadata.relation} -> $${outBinding} SET position = $${positionBinding};`,
        {
          [inBinding]: metadata.direction === "outgoing" ? write.owner : target,
          [outBinding]: metadata.direction === "outgoing" ? target : write.owner,
          [positionBinding]: position,
        },
      )
    }
  }
}

const encodeStored = (
  model: AnyModel,
  value: unknown,
  state: PlanContext,
  encodedOverride?: unknown,
): Effect.Effect<EncodedStorage, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    let encoded = encodedOverride === undefined
      ? yield* model.encode(value as never)
      : encodedOverride
    for (const plan of fieldPlans(model, "foreign")) {
      const target = targetForPlan(plan, state)
      if (target === undefined) continue
      const current = pathValue(value, plan.path)
      const encodedCurrent = fieldCarriesTargetSchema(fieldSchemaOf(plan.metadata), target)
        ? pathValue(encoded, plan.path)
        : undefined
      const stored = yield* foreignRecord(target, current, state, encodedCurrent)
      encoded = putPath(encoded, plan.path, stored)
    }
    const relationEncoded = new Map<string, unknown>()
    for (const plan of fieldPlans(model, "relate")) {
      relationEncoded.set(pathKey(plan.path), pathValue(encoded, plan.path))
      encoded = removePath(encoded, plan.path)
    }
    encoded = yield* secureValue(model, encoded, true)
    return { value: stripNullish(encoded), relationEncoded }
  })

const planModelWrite = (
  model: AnyModel,
  value: unknown,
  record: RecordId,
  mode: WriteMode,
  state: PlanContext,
  role: "parent" | "child",
  inputIndex?: number,
  encodedOverride?: unknown,
): Effect.Effect<void, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    const key = recordKey(record)
    if (state.planned.has(key)) return
    if (state.planning.has(key)) return
    state.planning.add(key)
    const encoded = yield* encodeStored(model, value, state, encodedOverride)
    const relationWrites = yield* prepareRelationWrites(model, value, record, state, encoded.relationEncoded)
    const content = isObject(encoded.value) ? { ...encoded.value } : encoded.value
    if (isObject(content) && model.idField !== undefined) delete content[model.idField]
    const prefix = `${role}_${state.planned.size}`
    const recordBinding = `${prefix}_record`
    const dataBinding = `${prefix}_data`
    const sql = mode === "create"
      ? `CREATE ONLY $${recordBinding} CONTENT $${dataBinding} RETURN AFTER;`
      : mode === "updateAt"
        ? `UPDATE $${recordBinding} CONTENT $${dataBinding} RETURN AFTER;`
        : `${mode === "createAt" ? "CREATE" : "UPSERT"} ONLY $${recordBinding} CONTENT $${dataBinding} RETURN AFTER;`
    state.builder.appendResult(sql, {
      [recordBinding]: record,
      [dataBinding]: content,
    }, {
      role,
      required: true,
      model,
      record,
      ...(inputIndex === undefined ? {} : { inputIndex }),
    })
    appendRelationWrites(state, relationWrites, prefix)
    state.planned.set(key, record)
    state.planning.delete(key)
  })

const executePlan = (
  state: PlanContext,
): Effect.Effect<readonly unknown[], RepositoryError, Database> => query(state.builder.finish().statement)

/**
 * Rebuild the encoded shape owned by `model` before decoding the model.
 *
 * A foreign target is a schema boundary. The target row is therefore restored
 * to the target schema's encoded caller shape, and the parent schema performs
 * the one enclosing decode. Returning a target's decoded Type here would make
 * a non-identity child codec run a second time when the parent decodes.
 */
const hydrateStoredEncoded = (
  model: AnyModel,
  row: unknown,
  state: PlannedState,
): Effect.Effect<unknown, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    const object = rowObject(row, "row decode")
    const record = rowRecord(model, object, "row decode")
    const key = rowKey(model, object, record)
    const known = state.hydratedEncoded.get(key)
    if (known !== undefined) return known
    // Preserve the existing recursive-row behavior for a cyclic graph. The
    // outer schema still owns decoding; a cycle cannot be recursively decoded
    // into a finite value without an explicit lazy schema representation.
    if (state.encoding.has(key)) return publicRow(model, object, record)
    state.encoding.add(key)
    let encodedValue: unknown = publicRow(model, object, record)
    for (const plan of fieldPlans(model, "foreign")) {
      const target = targetForPlan(plan, state)
      if (target === undefined) continue
      const stored = pathValue(object, plan.path)
      const encodedForeign = yield* hydrateForeign(target, stored, state)
      if (encodedForeign === undefined && astAllowsUndefined(plan.encodedAst)) {
        encodedValue = removePath(encodedValue, plan.path)
      } else {
        encodedValue = putPath(encodedValue, plan.path, encodedForeign)
      }
    }
    for (const plan of fieldPlans(model, "relate")) {
      if (record === undefined) {
        return yield* dbFailure<unknown>(new DbError({
          kind: DbErrorKind.InvalidModel,
          message: "SQL view relation hydration requires a RecordId in the query row",
          operation: "relation decode",
        }))
      }
      const relationValue = yield* hydrateRelation(model, record, plan, state)
      if (relationValue === undefined && relationAllowsUndefined(plan)) {
        encodedValue = removePath(encodedValue, plan.path)
      } else {
        encodedValue = putPath(encodedValue, plan.path, relationValue)
      }
    }
    encodedValue = yield* secureValue(model, encodedValue, false)
    state.encoding.delete(key)
    state.hydratedEncoded.set(key, encodedValue)
    return encodedValue
  })

const hydrateStored = (
  model: AnyModel,
  row: unknown,
  state: PlannedState,
): Effect.Effect<unknown, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    const object = rowObject(row, "row decode")
    const record = rowRecord(model, object, "row decode")
    const key = rowKey(model, object, record)
    const known = state.hydrated.get(key)
    if (known !== undefined) return known
    const encodedValue = yield* hydrateStoredEncoded(model, object, state)
    const decoded = yield* model.decode(encodedValue)
    state.hydrated.set(key, decoded)
    return decoded
  })

const fetchRecordRow = (
  model: AnyModel,
  record: RecordId,
): Effect.Effect<unknown, RepositoryError, Database> =>
  Effect.gen(function* () {
    const slots = yield* query(rawSql(
      "RETURN (SELECT *, record::id(id) AS id FROM ONLY $record);",
      { record: assertModelRecord(model, record, "get record") },
    ))
    const row = rawSlot(slots[0])
    if (row === undefined) {
      return yield* dbFailure<unknown>(missingRecordError(record))
    }
    return row
  })

const missingRecordError = (record: RecordId): DbError => new DbError({
  kind: DbErrorKind.NotFound,
  message: `record ${record.toString()} was not found`,
  operation: "get",
})

const isMissingRelationTableError = (error: DbError): boolean =>
  error.kind === DbErrorKind.MissingTable ||
  (error.kind === DbErrorKind.NotFound && /\btable\b.*\bdoes not exist\b/i.test(error.message))

const hydrateForeign = (
  model: AnyModel,
  value: unknown,
  state: PlannedState,
): Effect.Effect<unknown, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    if (value === null || value === undefined) return value
    if (Array.isArray(value)) {
      return yield* Effect.forEach(value, (item) => hydrateForeign(model, item, state), { concurrency: 1 })
    }
    const record = recordFromRaw(model, value, "foreign decode")
    const key = hydrationRowKey(model, record)
    if (state.batch?.missingRows.has(key)) {
      return yield* dbFailure<unknown>(missingRecordError(record))
    }
    const plannedRow = state.rawRows.get(key)
    const row = plannedRow === undefined ? yield* fetchRecordRow(model, record) : plannedRow
    if (plannedRow === undefined) state.rawRows.set(key, row)
    return yield* hydrateStoredEncoded(model, row, state)
  })

const HYDRATION_BATCH_SIZE = 5000

const batchState = (state: PlannedState): HydrationBatch => {
  if (state.batch !== undefined) return state.batch
  const batch: HydrationBatch = { relationRows: new Map(), missingRows: new Set() }
  state.batch = batch
  return batch
}

const nodeKey = (node: HydrationNode): string => hydrationRowKey(node.model, node.record)

const appendRecordCandidate = (
  groups: Map<string, TargetBatchGroup>,
  model: AnyModel,
  value: unknown,
  state: PlannedState,
): void => {
  if (value === null || value === undefined || model.source.kind === "sql") return
  let record: RecordId
  try {
    record = recordFromRaw(model, value, "hydration batch")
  } catch {
    // Keep malformed values on the ordinary hydration path. That path owns
    // the exact decode error and its operation label.
    return
  }
  const key = hydrationRowKey(model, record)
  if (state.rawRows.has(key) || state.batch?.missingRows.has(key)) return
  const groupKey = modelCacheKey(model)
  const group = groups.get(groupKey)
  if (group === undefined) {
    groups.set(groupKey, { model, records: new Map([[recordKey(record), record]]) })
  } else {
    group.records.set(recordKey(record), record)
  }
}

const collectForeignCandidates = (
  groups: Map<string, TargetBatchGroup>,
  model: AnyModel,
  value: unknown,
  state: PlannedState,
): void => {
  if (value === null || value === undefined) return
  if (Array.isArray(value)) {
    for (const item of value) collectForeignCandidates(groups, model, item, state)
    return
  }
  appendRecordCandidate(groups, model, value, state)
}

const relationTargetValue = (
  plan: StorageFieldPlan,
  value: unknown,
): unknown => {
  if (!isObject(value)) return undefined
  const metadata = plan.metadata.relate
  if (metadata === undefined) return undefined
  const field = metadata.direction === "outgoing" ? "out" : "in"
  return value[field]
}

const prefetchRelationRows = (
  nodes: readonly HydrationNode[],
  state: PlannedState,
): Effect.Effect<void, RepositoryError, Database> =>
  Effect.gen(function* () {
    const batch = batchState(state)
    const groups = new Map<string, RelationBatchGroup>()
    for (const node of nodes) {
      for (const plan of fieldPlans(node.model, "relate")) {
        const metadata = plan.metadata.relate
        if (metadata === undefined) continue
        let key: string
        try {
          validateIdentifier(metadata.relation, "relation name")
          key = `${modelCacheKey(node.model)}\u0000${metadata.relation}\u0000${metadata.direction}\u0000${pathKey(plan.path)}`
        } catch {
          // The ordinary hydration call retains the established validation
          // error and operation context.
          continue
        }
        const group = groups.get(key)
        const ownerKey = recordKey(node.record)
        if (group === undefined) {
          groups.set(key, { model: node.model, plan, owners: new Map([[ownerKey, node.record]]) })
        } else {
          group.owners.set(ownerKey, node.record)
        }
      }
    }

    for (const group of groups.values()) {
      const metadata = group.plan.metadata.relate
      if (metadata === undefined) continue
      const owners = [...group.owners.values()]
      const buckets = new Map<string, unknown[]>()
      let valid = true
      let missingTable = false
      for (let offset = 0; offset < owners.length; offset += HYDRATION_BATCH_SIZE) {
        const chunk = owners.slice(offset, offset + HYDRATION_BATCH_SIZE)
        const statement = metadata.direction === "outgoing"
          ? outgoingRowsForOwnersStatement(chunk, metadata.relation)
          : incomingRowsForOwnersStatement(chunk, metadata.relation)
        const slots = yield* query(statement).pipe(
          Effect.catch((error) => {
            if (isMissingRelationTableError(error)) {
              missingTable = true
              return Effect.succeed<readonly unknown[]>([[]])
            }
            return Effect.fail(error)
          }),
        )
        if (missingTable) break
        const rows = slots[0]
        if (!Array.isArray(rows)) {
          valid = false
          break
        }
        const requested = new Set(chunk.map(recordKey))
        for (const row of rows) {
          if (!isObject(row)) {
            valid = false
            break
          }
          const ownerValue = row.owner
          let owner: RecordId
          try {
            owner = recordFromRaw(group.model, ownerValue, "hydration batch relation owner")
          } catch {
            valid = false
            break
          }
          const ownerKey = recordKey(owner)
          if (!requested.has(ownerKey)) {
            valid = false
            break
          }
          const bucket = buckets.get(ownerKey)
          if (bucket === undefined) buckets.set(ownerKey, [row])
          else bucket.push(row)
        }
        if (!valid) break
      }

      if (missingTable) {
        for (const owner of owners) {
          batch.relationRows.set(relationBatchKey(group.model, owner, group.plan), [])
        }
        continue
      }
      if (!valid) continue
      for (const owner of owners) {
        batch.relationRows.set(
          relationBatchKey(group.model, owner, group.plan),
          buckets.get(recordKey(owner)) ?? [],
        )
      }
    }
  })

const prefetchTargetRows = (
  nodes: readonly HydrationNode[],
  state: PlannedState,
): Effect.Effect<readonly HydrationNode[], RepositoryError, Database> =>
  Effect.gen(function* () {
    const batch = batchState(state)
    const groups = new Map<string, TargetBatchGroup>()
    for (const node of nodes) {
      if (!isObject(node.row)) continue
      for (const plan of fieldPlans(node.model, "foreign")) {
        const target = targetForPlan(plan, state)
        if (target === undefined) continue
        collectForeignCandidates(groups, target, pathValue(node.row, plan.path), state)
      }
      for (const plan of fieldPlans(node.model, "relate")) {
        const target = targetForPlan(plan, state)
        if (target === undefined) continue
        const relationRowsForOwner = batch.relationRows.get(relationBatchKey(node.model, node.record, plan))
        if (relationRowsForOwner === undefined) continue
        // hydrateRelation validates every edge and scalar cardinality before
        // it hydrates a target. Validate the batch's raw edge shape without
        // publishing an error so malformed/wrong edges and scalar overflow
        // retain that same observable order on the ordinary path.
        const relationRecords: RecordId[] = []
        let valid = true
        for (const edge of relationRowsForOwner) {
          const value = relationTargetValue(plan, edge)
          if (value === undefined || Array.isArray(value)) {
            valid = false
            break
          }
          try {
            relationRecords.push(recordFromRaw(target, value, "hydration batch relation target"))
          } catch {
            valid = false
            break
          }
        }
        if (!valid || (!relationIsArray(plan) && relationRecords.length > 1)) continue
        for (const record of relationRecords) {
          appendRecordCandidate(groups, target, record, state)
        }
      }
    }

    for (const group of groups.values()) {
      const records = [...group.records.values()]
      for (let offset = 0; offset < records.length; offset += HYDRATION_BATCH_SIZE) {
        const chunk = records.slice(offset, offset + HYDRATION_BATCH_SIZE)
        const slots = yield* query(rawSql(
          "SELECT *, record::id(id) AS id FROM $table WHERE id IN $ids;",
          { table: new Table(modelTable(group.model)), ids: chunk },
        ))
        const rows = slots[0]
        if (!Array.isArray(rows)) continue
        const found = new Set<string>()
        let shapeValid = true
        for (const row of rows) {
          if (!isObject(row)) {
            shapeValid = false
            continue
          }
          let record: RecordId
          try {
            record = recordFromRaw(group.model, row.id, "hydration batch row")
          } catch {
            shapeValid = false
            continue
          }
          const key = recordKey(record)
          if (!group.records.has(key)) {
            shapeValid = false
            continue
          }
          found.add(key)
          state.rawRows.set(hydrationRowKey(group.model, record), row)
        }
        // Only an entirely well-shaped response proves absence. If the
        // engine returned a malformed row, leave it to the ordinary fetch so
        // that the established decode error remains observable.
        if (shapeValid) {
          for (const record of chunk) {
            if (!found.has(recordKey(record))) {
              batch.missingRows.add(hydrationRowKey(group.model, record))
            }
          }
        }
      }
    }

    const next: HydrationNode[] = []
    for (const group of groups.values()) {
      for (const record of group.records.values()) {
        const key = hydrationRowKey(group.model, record)
        const row = state.rawRows.get(key)
        if (row !== undefined) next.push({ model: group.model, record, row })
      }
    }
    return next
  })

const prefetchHydrationBatch = (
  roots: readonly HydrationNode[],
  state: PlannedState,
): Effect.Effect<void, RepositoryError, Database> =>
  Effect.gen(function* () {
    if (roots.length === 0) return
    const visited = new Set<string>()
    let frontier = roots
    while (frontier.length > 0) {
      const fresh = frontier.filter((node) => {
        const key = nodeKey(node)
        if (visited.has(key)) return false
        visited.add(key)
        return true
      })
      if (fresh.length === 0) return
      yield* prefetchRelationRows(fresh, state)
      frontier = yield* prefetchTargetRows(fresh, state)
    }
  })

const hydrateRelation = (
  model: AnyModel,
  owner: RecordId,
  plan: StorageFieldPlan,
  state: PlannedState,
): Effect.Effect<unknown, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    const metadata = plan.metadata.relate
    if (metadata === undefined) return undefined
    validateIdentifier(metadata.relation, "relation name")
    const anchor = metadata.direction === "outgoing" ? "in" : "out"
    const targetField = metadata.direction === "outgoing" ? "out" : "in"
    const cachedRows = state.batch?.relationRows.get(relationBatchKey(model, owner, plan))
    const slots = cachedRows === undefined
      ? yield* query(rawSql(
        `SELECT ${targetField}, position FROM ${metadata.relation} WHERE ${anchor} = $owner ORDER BY position ASC;`,
        { owner },
      )).pipe(
        Effect.catch((error) => isMissingRelationTableError(error)
          ? Effect.succeed<readonly unknown[]>([[]])
          : Effect.fail(error)),
      )
      : undefined
    const rawRowsForRelation = cachedRows ?? slots?.[0]
    if (!Array.isArray(rawRowsForRelation)) {
      return yield* dbFailure<unknown>(new DbError({
        kind: DbErrorKind.Decode,
        message: `relation ${metadata.relation} did not return a record-id array`,
        operation: "relation decode",
      }))
    }
    const target = targetForPlan(plan, state)
    if (target === undefined) {
      return yield* dbFailure<unknown>(new DbError({
        kind: DbErrorKind.InvalidModel,
        message: `relation ${metadata.relation} has no target model`,
        operation: "relation decode",
      }))
    }
    const records = rawRowsForRelation.map((value) => {
      if (!isObject(value)) {
        throw new DbError({
          kind: DbErrorKind.Decode,
          message: `relation ${metadata.relation} returned a malformed edge`,
          operation: "relation decode",
        })
      }
      return recordFromRaw(target, value[targetField], "relation decode")
    })
    if (!relationIsArray(plan) && records.length > 1) {
      return yield* dbFailure<unknown>(new DbError({
        kind: DbErrorKind.InvalidModel,
        message: `scalar relation ${metadata.relation} returned multiple edges`,
        operation: "relation decode",
      }))
    }
    const hydrated = yield* Effect.forEach(records, (record) => hydrateForeign(target, record, state), { concurrency: 1 })
    if (relationIsArray(plan)) return hydrated
    const first = hydrated[0]
    if (first !== undefined) return first
    if (relationAllowsNull(plan)) return null
    if (relationAllowsUndefined(plan)) return undefined
    return yield* dbFailure<unknown>(new DbError({
      kind: DbErrorKind.NotFound,
      message: `required relation ${metadata.relation} has no edge`,
      operation: "relation decode",
    }))
  })

const decodePlanParents = <M extends AnyModel>(
  model: M,
  slots: readonly unknown[],
  resultSlots: readonly ResultSlot[],
  state: PlannedState,
): Effect.Effect<readonly ModelValue<M>[], RepositoryError, Database | ModelServices<M>> =>
  Effect.gen(function* () {
    for (const result of resultSlots) {
      const row = rawSlot(slots[result.index])
      if (row === undefined) {
        return yield* dbFailure<readonly ModelValue<M>[]>(new DbError({
          kind: result.role === "parent" ? DbErrorKind.NotFound : DbErrorKind.EmptyResult,
          message: `write did not return ${result.role} row`,
          operation: "write",
        }))
      }
      state.rawRows.set(hydrationRowKey(result.model, result.record), row)
    }
    const parents = resultSlots.filter((result) => result.role === "parent")
    const roots: HydrationNode[] = []
    for (const result of parents) {
      const row = state.rawRows.get(hydrationRowKey(result.model, result.record))
      if (row !== undefined) roots.push({ model: result.model, record: result.record, row })
    }
    yield* prefetchHydrationBatch(roots, state)
    const output: ModelValue<M>[] = []
    for (const result of parents) {
      output.push(yield* hydrateStored(model, state.rawRows.get(hydrationRowKey(result.model, result.record)), state) as Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>)
    }
    return output
  })

const decodePlanView = <V extends AnyModel>(
  view: V,
  slots: readonly unknown[],
  resultSlots: readonly ResultSlot[],
  state: PlannedState,
): Effect.Effect<ModelValue<V>, RepositoryError, Database | ModelServices<V>> =>
  Effect.gen(function* () {
    for (const result of resultSlots) {
      const row = rawSlot(slots[result.index])
      if (row === undefined) {
        return yield* dbFailure<ModelValue<V>>(new DbError({
          kind: result.role === "parent" ? DbErrorKind.NotFound : DbErrorKind.EmptyResult,
          message: result.role === "parent" ? "write did not return a parent row" : "write did not return a child row",
          operation: "write.returning",
        }))
      }
      state.rawRows.set(hydrationRowKey(result.model, result.record), row)
    }
    const parent = resultSlots.find((result) => result.role === "parent")
    if (parent === undefined) {
      return yield* dbFailure<ModelValue<V>>(new DbError({
        kind: DbErrorKind.EmptyResult,
        message: "write did not return a parent row",
        operation: "write.returning",
      }))
    }
    const row = state.rawRows.get(hydrationRowKey(parent.model, parent.record))
    if (row === undefined) {
      return yield* dbFailure<ModelValue<V>>(new DbError({
        kind: DbErrorKind.NotFound,
        message: "write did not return a parent row",
        operation: "write.returning",
      }))
    }
    yield* prefetchHydrationBatch([{ model: view, record: parent.record, row }], state)
    return yield* hydrateStored(view, row, state) as Effect.Effect<ModelValue<V>, RepositoryError, ModelServices<V>>
  })

const preflightDuplicateIds = (
  model: AnyModel,
  values: readonly unknown[],
): Effect.Effect<void, DbError> =>
  sync("save_many preflight", () => {
    const seen = new Set<string>()
    for (const [index, value] of values.entries()) {
      const identity = stableIdentity(model, value)
      if (identity !== undefined && seen.has(identity)) {
        throw new DbError({
          kind: DbErrorKind.Conflict,
          message: `save_many received duplicate parent identity at input ${index}`,
          operation: "save_many",
        })
      }
      if (identity !== undefined) seen.add(identity)
    }
  })

const buildSinglePlan = (
  model: AnyModel,
  value: unknown,
  mode: WriteMode,
  record: RecordId,
  targets: WeakMap<object, AnyModel> = new WeakMap(),
): Effect.Effect<{ readonly state: PlanContext; readonly record: RecordId }, RepositoryError, Database | ModelServices<AnyModel>> =>
  Effect.gen(function* () {
    const state: PlanContext = {
      builder: new WritePlanBuilder(),
      planned: new Map(),
      rawRows: new Map(),
      hydrated: new Map(),
      hydratedEncoded: new Map(),
      hydrating: new Set(),
      encoding: new Set(),
      planning: new Set(),
      targets,
      batch: { relationRows: new Map(), missingRows: new Set() },
      rootModel: model,
      relationTables: new Set(),
    }
    yield* planModelWrite(model, value, record, mode, state, "parent", 0)
    return { state, record }
  })

const ensurePlainModel = (model: AnyModel, operation: string): Effect.Effect<void, DbError> =>
  fieldPlans(model, "foreign").length === 0 &&
  fieldPlans(model, "relate").length === 0 &&
  fieldPlans(model, "sensitive").length === 0
    ? Effect.succeed(undefined)
    : dbFailure(new DbError({
      kind: DbErrorKind.InvalidModel,
      message: `${operation} is supported only for plain models`,
      operation,
    }))

const makeStoreImpl = <M extends AnyModel>(model: M): Store<M> => {
  const localRecord = (id: RootId | RecordId, operation: string): Effect.Effect<RecordId, DbError> =>
    sync(operation, () => {
      if (isRecordId(id)) return assertModelRecord(model, id, operation)
      return model.recordId(assertRootId(id, operation))
    })

  const write = (
    value: ModelValue<M>,
    mode: WriteMode,
    recordEffect: (targets: WeakMap<object, AnyModel>) => Effect.Effect<RecordId, RepositoryError, Database | ModelServices<M>>,
  ): Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      const targets = new WeakMap<object, AnyModel>()
      const record = yield* recordEffect(targets)
      const plan = yield* buildSinglePlan(model, value, mode, record, targets)
      const slots = yield* executePlan(plan.state)
      const parents = yield* decodePlanParents(model, slots, plan.state.builder.finish().resultSlots, plan.state)
      const first = parents[0]
      if (first === undefined) {
        return yield* dbFailure<ModelValue<M>>(new DbError({
          kind: DbErrorKind.EmptyResult,
          message: "write returned no parent row",
          operation: mode,
        }))
      }
      return first
    }) as Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>

  const returning = <V extends ViewModel>(view: V): ReturningStore<M, V> => {
    const ensureOwnerView = (): Effect.Effect<void, DbError> => {
      if (
        view.source.kind === "view" &&
        view.source.owner === model &&
        view.source.ownerIdentity === model.source.ownerIdentity
      ) {
        return Effect.succeed(undefined)
      }
      return dbFailure(new DbError({
        kind: DbErrorKind.InvalidModel,
        message: "returning requires a table View owned by the written model",
        operation: "write.returning",
      }))
    }

    const writeReturning = (
      value: ModelValue<M>,
      mode: WriteMode,
      recordEffect: (targets: WeakMap<object, AnyModel>) => Effect.Effect<RecordId, RepositoryError, Database | ModelServices<M>>,
    ): Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>> =>
      Effect.gen(function* () {
        yield* ensureOwnerView()
        const targets = new WeakMap<object, AnyModel>()
        const record = yield* recordEffect(targets)
        const plan = yield* buildSinglePlan(model, value, mode, record, targets)
        const slots = yield* executePlan(plan.state)
        return yield* decodePlanView(
          view as AnyModel,
          slots,
          plan.state.builder.finish().resultSlots,
          plan.state,
        ) as Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>
      })

    const create = (value: ModelValue<M>) =>
      writeReturning(value, "create", () => Effect.succeed(generatedRecord(model)))

    const createAt = (id: RootId | RecordId, value: ModelValue<M>) =>
      writeReturning(value, "createAt", () => localRecord(id, "createAt"))

    const upsertAt = (id: RootId | RecordId, value: ModelValue<M>) =>
      writeReturning(value, "upsertAt", () => localRecord(id, "upsertAt"))

    const updateAt = (id: RootId | RecordId, value: ModelValue<M>) =>
      writeReturning(value, "updateAt", () => localRecord(id, "updateAt"))

    const save = (value: ModelValue<M>) =>
      writeReturning(value, "upsertAt", (targets) => modelRecord(model, value, true, true, targets)) as Effect.Effect<ViewModelValue<V>, RepositoryError, ReturningServices<M, V>>

    return { create, createAt, upsertAt, updateAt, save }
  }

  const getRecord = (id: RecordId): Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      const row = yield* fetchRecordRow(model, id)
      const state: PlannedState = {
        builder: new WritePlanBuilder(),
        planned: new Map(),
        rawRows: new Map([[hydrationRowKey(model, id), row]]),
        hydrated: new Map(),
        hydratedEncoded: new Map(),
        hydrating: new Set(),
        encoding: new Set(),
        planning: new Set(),
        targets: new WeakMap(),
      }
      return yield* hydrateStored(model, row, state) as Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
    })

  const get = (id: RootId | RecordId): Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      const record = yield* localRecord(id, "get")
      return yield* getRecord(record)
    })

  const list = (): Effect.Effect<readonly ModelValue<M>[], RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      const slots = yield* query(rawSql("SELECT *, record::id(id) AS id FROM $table;", {
        table: new Table(modelTable(model)),
      }))
      const rows = rawRows(slots[0])
      return yield* hydrateMany(model, rows) as Effect.Effect<readonly ModelValue<M>[], RepositoryError, ModelServices<M>>
    })

  const create = (value: ModelValue<M>): Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>> =>
    write(value, "create", () => Effect.succeed(generatedRecord(model)))

  const createAt = (id: RootId | RecordId, value: ModelValue<M>) =>
    write(value, "createAt", () => localRecord(id, "createAt"))

  const upsertAt = (id: RootId | RecordId, value: ModelValue<M>) =>
    write(value, "upsertAt", () => localRecord(id, "upsertAt"))

  const updateAt = (id: RootId | RecordId, value: ModelValue<M>) =>
    write(value, "updateAt", () => localRecord(id, "updateAt"))

  const save = (value: ModelValue<M>): Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>> =>
    write(value, "upsertAt", (targets) => modelRecord(model, value, true, true, targets))

  const saveMany = (
    values: readonly ModelValue<M>[],
  ): Effect.Effect<readonly ModelValue<M>[], RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      yield* preflightDuplicateIds(model, values)
      const output: ModelValue<M>[] = []
      const targets = new WeakMap<object, AnyModel>()
      for (let offset = 0; offset < values.length; offset += 5000) {
        const chunk = values.slice(offset, offset + 5000)
        const state: PlanContext = {
          builder: new WritePlanBuilder(),
          planned: new Map(),
          rawRows: new Map(),
          hydrated: new Map(),
          hydratedEncoded: new Map(),
          hydrating: new Set(),
          encoding: new Set(),
          planning: new Set(),
          targets,
          batch: { relationRows: new Map(), missingRows: new Set() },
          rootModel: model,
          relationTables: new Set(),
        }
        const records: RecordId[] = []
        for (const [index, value] of chunk.entries()) {
          // Batch parent identity keeps the pre-fallback policy: explicit ids
          // and declared unique fields may reuse a record, while id-less
          // fallback fields must not merge separate saveMany inputs.
          const record = yield* modelRecord(model, value, true, false, state.targets)
          records.push(record)
          yield* planModelWrite(model, value, record, "upsertAt", state, "parent", offset + index)
        }
        const slots = yield* executePlan(state)
        const resultSlots = state.builder.finish().resultSlots
        for (const result of resultSlots) {
          const row = rawSlot(slots[result.index])
          if (row !== undefined) state.rawRows.set(hydrationRowKey(result.model, result.record), row)
        }
        const parents = resultSlots
          .filter((result) => result.role === "parent")
          .sort((left, right) => (left.inputIndex ?? 0) - (right.inputIndex ?? 0))
        const roots: HydrationNode[] = []
        for (const result of parents) {
          const row = state.rawRows.get(hydrationRowKey(result.model, result.record))
          if (row !== undefined) roots.push({ model: result.model, record: result.record, row })
        }
        yield* prefetchHydrationBatch(roots, state)
        for (const result of parents) {
          const row = state.rawRows.get(hydrationRowKey(result.model, result.record))
          if (row === undefined) {
            return yield* dbFailure<readonly ModelValue<M>[]>(new DbError({
              kind: DbErrorKind.EmptyResult,
              message: "save_many did not return a parent row",
              operation: "save_many",
            }))
          }
          output.push(yield* hydrateStored(model, row, state) as Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>)
        }
      }
      return output
    })

  const exists = (): Effect.Effect<boolean, RepositoryError, ModelServices<M>> =>
    query(rawSql("SELECT VALUE id FROM $table LIMIT 1;", { table: new Table(modelTable(model)) })).pipe(
      Effect.map((slots) => rawRows(slots[0]).length > 0),
      Effect.catch((error) => error.kind === DbErrorKind.MissingTable ? Effect.succeed(false) : Effect.fail(error)),
    )

  const deleteRecord = (id: RecordId): Effect.Effect<void, RepositoryError, ModelServices<M>> =>
    query(rawSql("DELETE $record RETURN NONE;", { record: assertModelRecord(model, id, "delete") })).pipe(
      Effect.asVoid,
    )

  const deleteRoot = (id: RootId): Effect.Effect<void, RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () { yield* deleteRecord(yield* localRecord(id, "delete")) })

  const deleteAll = (): Effect.Effect<void, RepositoryError, ModelServices<M>> =>
    query(rawSql("DELETE $table RETURN NONE;", { table: new Table(modelTable(model)) })).pipe(
      Effect.asVoid,
      Effect.catch((error) => error.kind === DbErrorKind.MissingTable ? Effect.succeed(undefined) : Effect.fail(error)),
    )

  const listRecordIds = (): Effect.Effect<readonly RecordId[], RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      const slots = yield* query(rawSql("SELECT VALUE id FROM $table;", { table: new Table(modelTable(model)) }))
      return rawRows(slots[0]).map((id) => recordFromRaw(model, id, "list record ids"))
    })

  const findOneId = (field: string, value: string): Effect.Effect<RecordId, RepositoryError, ModelServices<M>> =>
    Effect.gen(function* () {
      validateIdentifier(field, "lookup field")
      if (field !== "id" && !model.fieldNames.includes(field)) {
        return yield* dbFailure<RecordId>(new DbError({
          kind: DbErrorKind.InvalidModel,
          message: `lookup field is not declared: ${field}`,
          operation: "findOneId",
        }))
      }
      const slots = yield* query(rawSql(
        `SELECT VALUE id FROM $table WHERE ${field === "id" ? "record::id(id)" : field} = $value LIMIT 2;`,
        { table: new Table(modelTable(model)), value },
      ))
      const ids = rawRows(slots[0])
      if (ids.length === 0) return yield* dbFailure<RecordId>(new DbError({ kind: DbErrorKind.NotFound, message: "lookup matched no records", operation: "findOneId" }))
      if (ids.length > 1) return yield* dbFailure<RecordId>(new DbError({ kind: DbErrorKind.InvalidModel, message: "lookup matched multiple records", operation: "findOneId" }))
      return recordFromRaw(model, ids[0], "findOneId")
    })

  const merge = (id: RootId | RecordId, value: Record<string, unknown>) =>
    Effect.gen(function* () {
      yield* ensurePlainModel(model, "merge")
      const record = yield* localRecord(id, "merge")
      const slots = yield* query(rawSql("UPDATE $record MERGE $data RETURN AFTER;", { record, data: value }))
      const row = rawSlot(slots[0])
      if (row === undefined) return yield* dbFailure<ModelValue<M>>(new DbError({ kind: DbErrorKind.NotFound, message: "merge target was not found", operation: "merge" }))
      const state: PlannedState = { builder: new WritePlanBuilder(), planned: new Map(), rawRows: new Map([[hydrationRowKey(model, record), row]]), hydrated: new Map(), hydratedEncoded: new Map(), hydrating: new Set(), encoding: new Set(), planning: new Set(), targets: new WeakMap() }
      return yield* hydrateStored(model, row, state) as Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
    })

  const patch = (id: RootId | RecordId, operations: readonly Patch[]) =>
    Effect.gen(function* () {
      yield* ensurePlainModel(model, "patch")
      const record = yield* localRecord(id, "patch")
      const slots = yield* query(rawSql("UPDATE $record PATCH $operations RETURN AFTER;", { record, operations }))
      const row = rawSlot(slots[0])
      if (row === undefined) return yield* dbFailure<ModelValue<M>>(new DbError({ kind: DbErrorKind.NotFound, message: "patch target was not found", operation: "patch" }))
      const state: PlannedState = { builder: new WritePlanBuilder(), planned: new Map(), rawRows: new Map([[hydrationRowKey(model, record), row]]), hydrated: new Map(), hydratedEncoded: new Map(), hydrating: new Set(), encoding: new Set(), planning: new Set(), targets: new WeakMap() }
      return yield* hydrateStored(model, row, state) as Effect.Effect<ModelValue<M>, RepositoryError, ModelServices<M>>
    })

  return { returning, create, createAt, upsertAt, updateAt, save, saveMany, get, getRecord, list, exists, delete: deleteRoot, deleteRecord, deleteAll, listRecordIds, findOneId, merge, patch }
}

/** Build a model-facing CRUD surface. The model remains the schema/value owner. */
export const makeStore = <Table extends string, C extends Schema.Top>(model: Model<Table, C>): Store<Model<Table, C>> =>
  makeStoreImpl(model)

const hydrateMany = (
  model: AnyModel,
  rows: readonly unknown[],
): Effect.Effect<readonly unknown[], RepositoryError, Database | Schema.Top["DecodingServices"]> =>
  Effect.gen(function* () {
    const state: PlannedState = {
      builder: new WritePlanBuilder(),
      planned: new Map(),
      rawRows: new Map(),
      hydrated: new Map(),
      hydratedEncoded: new Map(),
      hydrating: new Set(),
      encoding: new Set(),
      planning: new Set(),
      targets: new WeakMap(),
      batch: { relationRows: new Map(), missingRows: new Set() },
    }
    const roots: HydrationNode[] = []
    let allRootsValid = true
    for (const row of rows) {
      try {
        const object = rowObject(row, "row decode")
        const record = rowRecord(model, object, "row decode")
        if (record === undefined) {
          allRootsValid = false
          continue
        }
        state.rawRows.set(hydrationRowKey(model, record), row)
        roots.push({ model, record, row })
      } catch {
        // Let hydrateStored publish the established row error in input order.
        allRootsValid = false
      }
    }
    if (allRootsValid) yield* prefetchHydrationBatch(roots, state)
    const output: unknown[] = []
    for (const row of rows) {
      output.push(yield* hydrateStored(model, row, state))
    }
    return output
  })

/** Hydrate several rows with one operation-local relation/foreign read context. */
export const hydrateRows = <Table extends string, C extends Schema.Top, Params extends Schema.Top = Schema.Top>(
  model: Model<Table, C, Params>,
  rows: readonly unknown[],
): Effect.Effect<readonly C["Type"][], RepositoryError, Database | C["DecodingServices"]> =>
  hydrateMany(model as AnyModel, rows) as Effect.Effect<readonly C["Type"][], RepositoryError, Database | C["DecodingServices"]>

/** Public single-row decode seam; it uses the same hydrator as hydrateRows. */
export const hydrateRow = <Table extends string, C extends Schema.Top, Params extends Schema.Top = Schema.Top>(
  model: Model<Table, C, Params>,
  row: unknown,
): Effect.Effect<C["Type"], RepositoryError, Database | C["DecodingServices"]> =>
  hydrateRows(model, [row]).pipe(Effect.map((rows) => rows[0]!))

const viewProjection = (model: AnyModel): string => {
  const fields = model.fieldNames.map((field) => {
    validateIdentifier(field, "view field")
    return field
  }).filter((field) => field !== "id")
  return [...fields, "record::id(id) AS id"].join(", ")
}

/** Read-only table View surface; query methods share the public hydrateRows seam. */
export const makeView = <Table extends string, C extends Schema.Top, Params extends Schema.Top = Schema.Top>(
  model: Model<Table, C, Params>,
): ViewStore<Model<Table, C, Params>> => {
  const queryView = (params: ModelParams<Model<Table, C, Params>>["Type"]): Effect.Effect<readonly C["Type"][], RepositoryError, ViewServices<Model<Table, C, Params>>> =>
    Effect.gen(function* () {
      if (model.source.kind !== "sql") {
        return yield* dbFailure<readonly C["Type"][]>(new DbError({
          kind: DbErrorKind.InvalidModel,
          message: "only SQL views expose a typed query method",
          operation: "view.query",
        }))
      }

      const encoded = yield* Schema.encodeEffect(model.source.query.params)(params).pipe(
        Effect.mapError((cause) => decodeError(cause)),
      )
      let statement
      try {
        statement = rawSql(model.source.query.sql)
        if (model.source.query.bind !== undefined) {
          statement = model.source.query.bind(statement, encoded)
        } else if (encoded !== undefined) {
          if (!isObject(encoded) || Array.isArray(encoded)) {
            return yield* dbFailure<readonly C["Type"][]>(new DbError({
              kind: DbErrorKind.InvalidModel,
              message: "SQL view params must encode to a plain object when bind is omitted",
              operation: "view.query",
            }))
          }
          const bindings: Record<string, unknown> = {}
          for (const key of Object.keys(encoded)) bindings[key] = encoded[key]
          // BoundQuery treats an empty query as an empty constructor and drops
          // its bindings. A single whitespace keeps the SQL text unchanged in
          // meaning while forcing the SDK to merge the named parameters.
          statement = rawSql(model.source.query.sql, bindings)
        }
      } catch (cause) {
        return yield* dbFailure<readonly C["Type"][]>(asDbError(cause, "view.query bind"))
      }

      const slots = yield* query(statement)
      const result = slots[model.source.query.resultIndex]
      if (result === undefined) {
        return yield* dbFailure<readonly C["Type"][]>(new DbError({
          kind: DbErrorKind.EmptyResult,
          message: `SQL view result slot ${model.source.query.resultIndex} is missing`,
          operation: "view.query",
        }))
      }
      return yield* hydrateRows(model, rawRows(result))
    })

  const getRecord = (id: RecordId): Effect.Effect<C["Type"], RepositoryError, Database | C["DecodingServices"]> =>
    Effect.gen(function* () {
      if (model.source.kind !== "view" || model.source.readSource !== "table") {
        return yield* dbFailure<C["Type"]>(new DbError({ kind: DbErrorKind.InvalidModel, message: "SQL Views require a typed query method", operation: "view.get" }))
      }
      const slots = yield* query(rawSql(
        `RETURN (SELECT ${viewProjection(model)} FROM ONLY $record);`,
        { record: assertModelRecord(model, id, "view get") },
      ))
      const row = rawSlot(slots[0])
      if (row === undefined) return yield* dbFailure<C["Type"]>(new DbError({ kind: DbErrorKind.NotFound, message: "view target was not found", operation: "view.get" }))
      return yield* hydrateRow(model, row)
    })

  const get = (id: RootId | RecordId) =>
    Effect.gen(function* () {
      if (model.source.kind !== "view" || model.source.readSource !== "table") {
        return yield* dbFailure<C["Type"]>(new DbError({ kind: DbErrorKind.InvalidModel, message: "SQL Views require a typed query method", operation: "view.get" }))
      }
      const record = isRecordId(id) ? id : model.recordId(assertRootId(id, "view.get"))
      return yield* getRecord(record)
    })

  const list = (): Effect.Effect<readonly C["Type"][], RepositoryError, Database | C["DecodingServices"]> =>
    Effect.gen(function* () {
      if (model.source.kind !== "view" || model.source.readSource !== "table") {
        return yield* dbFailure<readonly C["Type"][]>(new DbError({ kind: DbErrorKind.InvalidModel, message: "SQL Views require a typed query method", operation: "view.list" }))
      }
      const slots = yield* query(rawSql(
        `SELECT ${viewProjection(model)} FROM $table;`,
        { table: new Table(modelTable(model)) },
      ))
      return yield* hydrateRows(model, rawRows(slots[0]))
    })

  return { query: queryView, get, list }
}
