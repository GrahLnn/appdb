import { Effect, Encoding, Result, Schema } from "effect"
import { RecordId, Table } from "surrealdb"

import { DbError, DbErrorKind, isDbError } from "./errors.js"
import { assertRootId, isRecordId } from "./id.js"
import type { Model } from "./model.js"
import { hydrateRows, type RepositoryError } from "./repository.js"
import { query, rawSql, validateIdentifier, type Order, type RawSql } from "./query.js"

/** A keyset page returned by a model or relation reader. */
export interface Page<T> {
  readonly items: readonly T[]
  readonly next?: PageCursor
}

/** Values which can be carried by the Rust-compatible keyset cursor. */
export type CursorValue = string | number | bigint | boolean | null | RecordId

type CursorOrder = "asc" | "desc"

interface EncodedScalar {
  readonly kind: "null" | "boolean" | "string" | "number" | "bigint"
  readonly value?: boolean | string | number
}

interface EncodedRecord {
  readonly kind: "record"
  readonly table: string
  readonly id: EncodedScalar
}

type EncodedValue = EncodedScalar | EncodedRecord

interface CursorPayload {
  readonly version: 1
  readonly table: string
  readonly field: string
  readonly order: CursorOrder
  readonly value: EncodedValue
  readonly id: EncodedValue
}

const CURSOR_VERSION = 1 as const

const invalidCursor = (message: string, cause?: unknown): DbError =>
  new DbError({
    kind: DbErrorKind.Decode,
    message,
    operation: "page cursor",
    ...(cause === undefined ? {} : { cause }),
  })

const encodeBase64Url = (input: string): string => Encoding.encodeBase64Url(input)

const decodeBase64Url = (input: string): string => {
  // Encoding accepts padded, CR/LF-containing, and empty Base64Url strings.
  // Cursor v1 intentionally admits only canonical non-empty unpadded tokens.
  if (input.length === 0 || !/^[A-Za-z0-9_-]+$/.test(input)) {
    throw invalidCursor("page cursor is not valid base64url")
  }
  const decoded = Encoding.decodeBase64Url(input)
  if (Result.isFailure(decoded)) {
    throw invalidCursor("page cursor contains invalid base64url digits", decoded.failure)
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(decoded.success)
  } catch (cause) {
    throw invalidCursor("page cursor is not valid UTF-8", cause)
  }
}

const encodeValue = (value: CursorValue, label: string): EncodedValue => {
  if (value === null) return { kind: "null" }
  if (typeof value === "boolean") return { kind: "boolean", value }
  if (typeof value === "string") return { kind: "string", value }
  if (typeof value === "bigint") return { kind: "bigint", value: value.toString(10) }
  if (typeof value === "number") {
    if (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value))) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `${label} must be a finite safe number` })
    }
    return { kind: "number", value }
  }
  if (isRecordId(value)) {
    validateIdentifier(value.table.name, `${label} table`)
    return {
      kind: "record",
      table: value.table.name,
      id: encodeValue(value.id as CursorValue, `${label} id`) as EncodedScalar,
    }
  }
  throw new DbError({ kind: DbErrorKind.Decode, message: `${label} must be a scalar or RecordId` })
}

const isEncodedScalar = (value: unknown): value is EncodedScalar => {
  if (typeof value !== "object" || value === null || !("kind" in value)) return false
  const kind = (value as { kind?: unknown }).kind
  if (kind === "null") return !Object.hasOwn(value, "value")
  if (kind === "boolean") return typeof (value as { value?: unknown }).value === "boolean"
  if (kind === "string" || kind === "bigint") return typeof (value as { value?: unknown }).value === "string"
  if (kind === "number") {
    const number = (value as { value?: unknown }).value
    return typeof number === "number" && Number.isFinite(number) &&
      (!Number.isInteger(number) || Number.isSafeInteger(number))
  }
  return false
}

const decodeValue = (value: unknown, label: string): CursorValue => {
  if (isEncodedScalar(value)) {
    switch (value.kind) {
      case "null": return null
      case "boolean": return value.value as boolean
      case "string": return value.value as string
      case "number": return value.value as number
      case "bigint":
        try {
          return BigInt(value.value as string)
        } catch (cause) {
          throw invalidCursor(`${label} bigint payload is malformed`, cause)
        }
    }
  }
  if (typeof value === "object" && value !== null && (value as { kind?: unknown }).kind === "record") {
    const record = value as { kind: "record"; table?: unknown; id?: unknown }
    if (typeof record.table !== "string" || !isEncodedScalar(record.id)) {
      throw invalidCursor(`${label} record payload is malformed`)
    }
    validateIdentifier(record.table, `${label} table`)
    const id = decodeValue(record.id, `${label} id`)
    if (id === null || typeof id === "boolean" || typeof id === "object") {
      throw invalidCursor(`${label} record id must be a string, number, or bigint`)
    }
    return new RecordId(record.table, id)
  }
  throw invalidCursor(`${label} payload is malformed`)
}

const encodePayload = (payload: CursorPayload): string =>
  encodeBase64Url(JSON.stringify(payload))

const decodePayload = (token: string): CursorPayload => {
  let decoded: unknown
  try {
    decoded = JSON.parse(decodeBase64Url(token))
  } catch (cause) {
    if (isDbError(cause)) throw cause
    throw invalidCursor("page cursor JSON is malformed", cause)
  }
  if (typeof decoded !== "object" || decoded === null) {
    throw invalidCursor("page cursor payload is not an object")
  }
  const payload = decoded as Partial<CursorPayload>
  if (payload.version !== CURSOR_VERSION || typeof payload.table !== "string" ||
      typeof payload.field !== "string" || (payload.order !== "asc" && payload.order !== "desc")) {
    throw invalidCursor("unsupported page cursor version or metadata")
  }
  validateIdentifier(payload.table, "page cursor table")
  validateIdentifier(payload.field, "page cursor field")
  // Decode now so malformed values fail at the cursor boundary, rather than
  // after a query has already been sent.
  decodeValue(payload.value, "page cursor value")
  decodeValue(payload.id, "page cursor id")
  return payload as CursorPayload
}

/** Opaque, versioned cursor used by keyset pagination. */
export class PageCursor {
  readonly #token: string
  readonly #payload: CursorPayload

  private constructor(token: string, payload: CursorPayload) {
    this.#token = token
    this.#payload = payload
  }

  static fromString(token: string): PageCursor {
    if (typeof token !== "string") throw invalidCursor("page cursor must be a string")
    return new PageCursor(token, decodePayload(token))
  }

  static decode(token: string): PageCursor {
    return PageCursor.fromString(token)
  }

  static fromToken(token: string): PageCursor {
    return PageCursor.fromString(token)
  }

  static create(payload: {
    readonly table: string
    readonly field: string
    readonly order: Order
    readonly value: CursorValue
    readonly id: CursorValue
  }): PageCursor {
    validateIdentifier(payload.table, "page cursor table")
    validateIdentifier(payload.field, "page cursor field")
    if (payload.order !== "asc" && payload.order !== "desc") {
      throw invalidCursor("page cursor order must be `asc` or `desc`")
    }
    const normalized: CursorPayload = {
      version: CURSOR_VERSION,
      table: payload.table,
      field: payload.field,
      order: payload.order,
      value: encodeValue(payload.value, "page cursor value"),
      id: encodeValue(payload.id, "page cursor id"),
    }
    return new PageCursor(encodePayload(normalized), normalized)
  }

  asString(): string {
    return this.#token
  }

  intoInner(): string {
    return this.#token
  }

  toString(): string {
    return this.#token
  }

  toJSON(): string {
    return this.#token
  }

  get table(): string {
    return this.#payload.table
  }

  get field(): string {
    return this.#payload.field
  }

  get order(): CursorOrder {
    return this.#payload.order
  }

  get value(): CursorValue {
    return decodeValue(this.#payload.value, "page cursor value")
  }

  get id(): CursorValue {
    return decodeValue(this.#payload.id, "page cursor id")
  }
}

export type PageCursorInput = PageCursor | string

const normalizeCursor = (cursor: PageCursorInput): PageCursor =>
  cursor instanceof PageCursor ? cursor : PageCursor.fromString(cursor)

export interface PaginationModel<Table extends string = string, C extends Schema.Top = Schema.Top> {
  readonly model: Model<Table, C>
  readonly field: string
  readonly order: Order
}

const normalizeOrder = (order: Order): CursorOrder => {
  if (order !== "asc" && order !== "desc") {
    throw new DbError({ kind: DbErrorKind.InvalidModel, message: "pagination order must be `asc` or `desc`" })
  }
  return order
}

const positiveCount = (count: number): number => {
  if (!Number.isSafeInteger(count) || count <= 0) {
    throw new DbError({ kind: DbErrorKind.InvalidModel, message: "pagination count must be a positive safe integer" })
  }
  if (count === Number.MAX_SAFE_INTEGER) {
    throw new DbError({ kind: DbErrorKind.InvalidModel, message: "pagination count overflowed the lookahead window" })
  }
  return count
}

/** Immutable keyset plan; it owns cursor/table/field compatibility checks. */
export class PaginationPlan<Table extends string, C extends Schema.Top> {
  readonly model: Model<Table, C>
  readonly field: string
  readonly order: CursorOrder

  constructor(model: Model<Table, C>, field: string, order: Order) {
    this.model = model
    this.field = model.fieldNames.includes(field) || field === "id" ? field : (() => {
      throw new DbError({ kind: DbErrorKind.InvalidModel, message: `pagination field is not declared: ${field}` })
    })()
    this.order = normalizeOrder(order)
    validateIdentifier(this.field, "pagination field")
  }

  buildStatement(count: number, cursor?: PageCursorInput): RawSql {
    const requested = positiveCount(count)
    const bindings: Record<string, unknown> = {
      table: new Table(this.model.table),
      count: requested,
    }
    const key = this.field === "id" ? "__page_public_id" : this.field
    const direction = this.order.toUpperCase()
    let statement = "LET $rows = (SELECT *, id AS __page_record, record::id(id) AS __page_public_id FROM $table); "
    if (cursor !== undefined) {
      const decoded = normalizeCursor(cursor)
      this.assertCursor(decoded)
      bindings.cursor_value = decoded.value
      const id = decoded.id
      if (!isRecordId(id)) {
        throw new DbError({ kind: DbErrorKind.Decode, message: "page cursor id is not a RecordId" })
      }
      bindings.cursor_record = id
      const than = this.order === "asc" ? ">" : "<"
      statement += `SELECT *, __page_public_id AS id FROM $rows WHERE (${key} ${than} $cursor_value OR (${key} = $cursor_value AND __page_record ${than} $cursor_record)) ORDER BY ${key} ${direction}, __page_record ${direction} LIMIT $count;`
    } else {
      statement += `SELECT *, __page_public_id AS id FROM $rows ORDER BY ${key} ${direction}, __page_record ${direction} LIMIT $count;`
    }
    return rawSql(statement, bindings)
  }

  buildCursor(row: unknown): PageCursor {
    if (typeof row !== "object" || row === null) {
      throw new DbError({ kind: DbErrorKind.Decode, message: "pagination row must be an object" })
    }
    const source = row as Record<string, unknown>
    const value = source[this.field]
    if (value === undefined) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `stored row is missing pagination field \`${this.field}\`` })
    }
    const id = source.id
    if (id === undefined) {
      throw new DbError({ kind: DbErrorKind.Decode, message: "stored row is missing `id`" })
    }
    const record = isRecordId(id)
      ? (() => {
          if (id.table.name !== this.model.table) {
            throw new DbError({ kind: DbErrorKind.InvalidIdentifier, message: `pagination row id belongs to table ${id.table.name}, expected ${this.model.table}` })
          }
          return assertRootId(id.id, "pagination row id")
        })()
      : assertRootId(id, "pagination row id")
    return PageCursor.create({
      table: this.model.table,
      field: this.field,
      order: this.order,
      value: value as CursorValue,
      id: this.model.recordId(record),
    })
  }

  assertCursor(cursor: PageCursor): void {
    if (cursor.table !== this.model.table) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `page cursor targets table \`${cursor.table}\` but \`${this.model.table}\` was requested` })
    }
    if (cursor.field !== this.field) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `page cursor targets field \`${cursor.field}\` but \`${this.field}\` was requested` })
    }
    if (cursor.order !== this.order) {
      throw new DbError({ kind: DbErrorKind.Decode, message: "page cursor order does not match the requested pagination direction" })
    }
    const id = cursor.id
    if (!isRecordId(id) || id.table.name !== this.model.table) {
      throw new DbError({ kind: DbErrorKind.Decode, message: "page cursor id does not belong to the requested model table" })
    }
    // This validates arrays/objects and rejects values that cannot be bound by
    // the scalar keyset predicate before the query is sent.
    encodeValue(cursor.value, "page cursor value")
  }
}

/** Build the canonical table keyset statement for a model. */
export const buildPaginationQuery = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  count: number,
  cursor: PageCursorInput | undefined,
  order: Order,
  field = model.paginationField,
): RawSql => {
  if (field === undefined) {
    throw new DbError({ kind: DbErrorKind.InvalidModel, message: `model \`${model.table}\` does not declare a pagination field` })
  }
  return new PaginationPlan(model, field, order).buildStatement(count, cursor)
}

/** Turn a lookahead result into a page and build a cursor from its last row. */
export const pageFromRows = <T, Table extends string, C extends Schema.Top>(
  plan: PaginationPlan<Table, C>,
  rows: readonly T[],
  requestedCount: number,
): Page<T> => {
  const count = positiveCount(requestedCount)
  if (rows.length <= count) return { items: rows }
  const items = rows.slice(0, count)
  const last = items[items.length - 1]
  if (last === undefined) throw new DbError({ kind: DbErrorKind.EmptyResult, message: "pagination lookahead lost its last row" })
  return { items, next: plan.buildCursor(last) }
}

/** Execute a raw keyset page and return rows without bypassing the query owner. */
export const queryPage = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  count: number,
  cursor: PageCursorInput | undefined,
  order: Order,
  field = model.paginationField,
): Effect.Effect<Page<unknown>, DbError, import("./connection.js").Database> =>
  Effect.gen(function* () {
    if (field === undefined) {
      return yield* Effect.fail(new DbError({ kind: DbErrorKind.InvalidModel, message: `model \`${model.table}\` does not declare a pagination field` }))
    }
    const plan = new PaginationPlan(model, field, order)
    const rows = yield* query(plan.buildStatement(count + 1, cursor))
    const value = rows[1]
    if (!Array.isArray(value)) {
      return yield* Effect.fail(new DbError({ kind: DbErrorKind.Decode, message: "pagination query returned no row array" }))
    }
    return pageFromRows(plan, value, count)
  })

/**
 * Execute a keyset page and hydrate each row through the repository owner.
 * Cursor construction happens from the storage row before codec hydration so
 * encoded pagination values remain the values used by the SQL comparison.
 */
export const paginate = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  count: number,
  cursor: PageCursorInput | undefined,
  order: Order,
  field = model.paginationField,
): Effect.Effect<Page<C["Type"]>, RepositoryError, import("./connection.js").Database | C["DecodingServices"]> =>
  Effect.gen(function* () {
    if (field === undefined) {
      return yield* Effect.fail(new DbError({ kind: DbErrorKind.InvalidModel, message: `model \`${model.table}\` does not declare a pagination field` }))
    }
    const plan = new PaginationPlan(model, field, order)
    const slots = yield* query(plan.buildStatement(positiveCount(count) + 1, cursor))
    const raw = slots[1]
    if (!Array.isArray(raw)) {
      return yield* Effect.fail(new DbError({ kind: DbErrorKind.Decode, message: "pagination query returned no row array" }))
    }
    const rawPage = pageFromRows(plan, raw, count)
    const items = yield* hydrateRows(model, rawPage.items)
    return rawPage.next === undefined
      ? { items }
      : { items, next: rawPage.next }
  })

export const paginAsc = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  count: number,
  cursor?: PageCursorInput,
  field = model.paginationField,
) => paginate(model, count, cursor, "asc", field)

export const paginDesc = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  count: number,
  cursor?: PageCursorInput,
  field = model.paginationField,
) => paginate(model, count, cursor, "desc", field)
