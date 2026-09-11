import { BoundQuery, Table } from "surrealdb"
import { Effect, Schema } from "effect"

import { dbErrorFromCause, DbError, isRollbackPlaceholder } from "./errors.js"
import { Database, type DatabaseResponse } from "./connection.js"

/** A SurrealQL statement together with its named bindings and result slots. */
export type RawSql<R extends unknown[] = unknown[]> = BoundQuery<R>

/** Values which can safely be used by the model predicate builder. */
export type ScalarValue = string | number | bigint | boolean | null

/** Sort direction shared by ordered scans and keyset pagination. */
export type Order = "asc" | "desc"

/** A small, bounded predicate language for model reads. */
export type Predicate =
  | {
      readonly field: string
      readonly op: "eq" | "ne" | "lt" | "lte" | "gt" | "gte"
      readonly value: ScalarValue
    }
  | { readonly and: readonly Predicate[] }
  | { readonly or: readonly Predicate[] }
  | { readonly not: Predicate }

export interface ModelQuerySpec {
  readonly table: string
  /** Declared model fields. `id` is admitted separately as the record identity. */
  readonly fields: readonly string[]
  readonly predicate?: Predicate
  readonly orderBy?: { readonly field: string; readonly order: Order }
  readonly limit?: number
  readonly offset?: number
}

export const IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/

const invalid = (message: string): DbError =>
  new DbError({ kind: "InvalidIdentifier", message })

/** Validates the plain identifier subset used by Rust relation and field paths. */
export const validateIdentifier = (value: string, label = "identifier"): string => {
  if (!IDENTIFIER.test(value)) {
    throw invalid(`${label} must be a plain SurrealQL identifier: ${value}`)
  }
  return value
}

export const assertModelField = (
  field: string,
  declaredFields: readonly string[],
): string => {
  validateIdentifier(field, "model field")
  if (field !== "id" && !declaredFields.includes(field)) {
    throw new DbError({
      kind: "InvalidModel",
      message: `model field is not declared: ${field}`,
    })
  }
  return field
}

const ensureSafeScalar = (value: ScalarValue, label: string): ScalarValue => {
  if (typeof value === "number" && (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value)))) {
    throw new DbError({
      kind: "Decode",
      message: `${label} must be a finite safe number or bigint`,
    })
  }
  return value
}

/** Creates the canonical SDK representation without another interpolation layer. */
export const rawSql = <R extends unknown[] = unknown[]>(
  sql: string,
  bindings: Readonly<Record<string, unknown>> = {},
): RawSql<R> => new BoundQuery<R>(sql, { ...bindings })

/** `surql` remains the official tagged representation; this alias is only ergonomic. */
export { surql as sql } from "surrealdb"

export const appendSql = <R extends unknown[]>(base: RawSql<R>, part: RawSql<R> | string): RawSql<R> => {
  const next = new BoundQuery<R>(base)
  if (typeof part === "string") {
    next.append(part)
  } else {
    next.append(part)
  }
  return next
}

const predicateSql = (
  predicate: Predicate,
  declaredFields: readonly string[],
  bindings: Record<string, unknown>,
  nextIndex: { value: number },
): string => {
  if ("field" in predicate) {
    const field = assertModelField(predicate.field, declaredFields)
    const value = ensureSafeScalar(predicate.value, `predicate value for ${field}`)
    const name = `predicate_${nextIndex.value++}`
    bindings[name] = value
    const lhs = field === "id" ? "record::id(id)" : field
    const operator = predicate.op === "eq"
      ? "="
      : predicate.op === "ne"
        ? "!="
        : predicate.op === "lt"
          ? "<"
          : predicate.op === "lte"
            ? "<="
            : predicate.op === "gt"
              ? ">"
              : ">="
    return `${lhs} ${operator} $${name}`
  }

  if ("and" in predicate) {
    return predicate.and.length === 0
      ? "true"
      : `(${predicate.and.map((item) => predicateSql(item, declaredFields, bindings, nextIndex)).join(" AND ")})`
  }

  if ("or" in predicate) {
    return predicate.or.length === 0
      ? "false"
      : `(${predicate.or.map((item) => predicateSql(item, declaredFields, bindings, nextIndex)).join(" OR ")})`
  }

  return `(NOT ${predicateSql(predicate.not, declaredFields, bindings, nextIndex)})`
}

/** Builds a bounded model read. Values are named bindings; identifiers are validated/allowlisted. */
export const buildModelQuery = <R extends unknown[] = unknown[]>(
  spec: ModelQuerySpec,
): RawSql<R> => {
  validateIdentifier(spec.table, "model table")
  const table = new Table(spec.table)
  const bindings: Record<string, unknown> = { table }
  const clauses: string[] = []
  const nextIndex = { value: 0 }

  if (spec.predicate !== undefined) {
    clauses.push(predicateSql(spec.predicate, spec.fields, bindings, nextIndex))
  }

  let query = "SELECT *, record::id(id) AS id FROM $table"
  if (clauses.length > 0) query += ` WHERE ${clauses.join(" AND ")}`

  if (spec.orderBy !== undefined) {
    const field = assertModelField(spec.orderBy.field, spec.fields)
    if (spec.orderBy.order !== "asc" && spec.orderBy.order !== "desc") {
      throw new DbError({ kind: "InvalidModel", message: "query order must be `asc` or `desc`" })
    }
    const key = field === "id" ? "record::id(id)" : field
    query += ` ORDER BY ${key} ${spec.orderBy.order.toUpperCase()}`
  }

  if (spec.offset !== undefined) {
    if (!Number.isSafeInteger(spec.offset) || spec.offset < 0) {
      throw new DbError({ kind: "InvalidModel", message: "query offset must be a non-negative safe integer" })
    }
    bindings.offset = spec.offset
    query += " START $offset"
  }

  // SurrealQL's range modifiers are ordered as START then LIMIT. Keeping
  // this order also matches the Rust query builders when both bounds exist.
  if (spec.limit !== undefined) {
    if (!Number.isSafeInteger(spec.limit) || spec.limit < 0) {
      throw new DbError({ kind: "InvalidModel", message: "query limit must be a non-negative safe integer" })
    }
    bindings.limit = spec.limit
    query += " LIMIT $limit"
  }

  return rawSql<R>(`${query};`, bindings)
}

/** Executes a checked query through the owner-provided Database service. */
export const query = <R extends unknown[] = unknown[]>(
  statement: RawSql<R> | string,
  bindings?: Readonly<Record<string, unknown>>,
): Effect.Effect<Readonly<R>, DbError, Database> =>
  Effect.gen(function* () {
    const database = yield* Database
    const slots = yield* (typeof statement === "string"
      ? database.query(statement, bindings)
      : database.query(statement))
    return slots as Readonly<R>
  })

/** Executes a query and exposes native per-statement responses without checking them. */
export const queryUnchecked = (
  statement: RawSql | string,
  bindings?: Readonly<Record<string, unknown>>,
): Effect.Effect<ReadonlyArray<DatabaseResponse>, DbError, Database> =>
  Effect.gen(function* () {
    const database = yield* Database
    if (typeof statement === "string") {
      return yield* database.queryUnchecked(statement, bindings)
    }
    return yield* database.queryUnchecked(statement)
  })

export type FailedResponse = Extract<DatabaseResponse, { success: false }>

/** A failed response together with its original query slot index. */
export interface ResponseFailure {
  readonly index: number
  readonly response: FailedResponse
}

/**
 * Select the response that explains a failed batch.
 *
 * SurrealDB emits `QueryError(NotExecuted)` responses for statements skipped
 * after an earlier statement failed. Those placeholders preserve response
 * cardinality but do not identify the cause. Return the first real failure,
 * falling back to the first placeholder only when no real failure exists.
 */
export const selectResponseFailure = (
  responses: readonly DatabaseResponse[],
): ResponseFailure | undefined => {
  let placeholder: ResponseFailure | undefined
  for (const [index, response] of responses.entries()) {
    if (response.success) continue
    const failure: ResponseFailure = { index, response }
    if (!isRollbackPlaceholder(response.error)) return failure
    placeholder ??= failure
  }
  return placeholder
}

/**
 * Validate native responses and project successful values back to raw slots.
 * The response count and order remain unchanged; failures are reported using
 * the selected real cause rather than a transaction-skipped placeholder.
 */
export const checkedResponseSlots = (
  responses: readonly DatabaseResponse[],
  operation = "query",
): Effect.Effect<ReadonlyArray<unknown>, DbError> =>
  Effect.suspend(() => {
    const failure = selectResponseFailure(responses)
    if (failure !== undefined) {
      return Effect.fail(
        dbErrorFromCause(
          `${operation} result ${failure.index}`,
          failure.response.error,
        ),
      )
    }
    return Effect.succeed(
      responses.map((response) => response.success ? response.result : undefined),
    )
  })

/** Explicit name for the checked raw-slot path. */
export const queryChecked = query

/** Explicit name for the native SDK response path. */
export const queryRaw = queryUnchecked

/** Decodes one raw result slot with the original Effect Schema codec. */
export const decodeSlot = <S extends Schema.ConstraintDecoder<unknown, unknown>>(
  schema: S,
  slots: readonly unknown[],
  index = 0,
): Effect.Effect<S["Type"], DbError, S["DecodingServices"]> =>
  Effect.suspend(() => {
    if (index < 0 || index >= slots.length) {
      return Effect.fail(
        new DbError({
          kind: "EmptyResult",
          message: `query result slot ${index} is missing`,
          operation: "decodeSlot",
        }),
      )
    }
    return Schema.decodeUnknownEffect(schema)(slots[index]).pipe(
      Effect.mapError(
        (cause) => new DbError({ kind: "Decode", message: `failed to decode query slot ${index}`, cause }),
      ),
    )
  })

/** Decode an array-valued result slot with the supplied item schema. */
export const queryTake = <S extends Schema.ConstraintDecoder<unknown, unknown>>(
  statement: RawSql | string,
  schema: S,
  index = 0,
  bindings?: Readonly<Record<string, unknown>>,
): Effect.Effect<ReadonlyArray<S["Type"]>, DbError, Database | S["DecodingServices"]> =>
  query(statement, bindings).pipe(
    Effect.flatMap((slots) => decodeSlot(Schema.Array(schema), slots, index)),
  )

/** Decode an optional value from one checked result slot. */
export const queryReturn = <S extends Schema.ConstraintDecoder<unknown, unknown>>(
  statement: RawSql | string,
  schema: S,
  index = 0,
  bindings?: Readonly<Record<string, unknown>>,
): Effect.Effect<S["Type"] | undefined, DbError, Database | S["DecodingServices"]> =>
  query(statement, bindings).pipe(
    Effect.flatMap((slots) => {
      if (index < 0 || index >= slots.length || slots[index] === undefined) {
        return Effect.succeed<S["Type"] | undefined>(undefined)
      }
      return decodeSlot(schema, slots, index)
    }),
  )

/** Bound-query aliases mirror the Rust raw/bound helper split. */
export const queryBound = queryRaw
export const queryBoundChecked = queryChecked
export const queryBoundTake = queryTake
export const queryBoundReturn = queryReturn
