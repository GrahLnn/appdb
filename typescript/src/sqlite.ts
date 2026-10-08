import { Effect, Result, Schema } from "effect"
import { Base64Url } from "effect/encoding"
import type { Model } from "./model.js"
import { assertRootId, type RootId } from "./id.js"
import { DbError, DbErrorKind, dbErrorFromCause, errorMessage } from "./errors.js"

/** Values accepted by a SQLite driver parameter binding. */
export type SqliteParam = string | number | bigint | Uint8Array | null

/** Result returned by a driver's write operation. */
export interface SqliteRunResult {
  readonly changes: number
}

/** Small driver boundary; `better-sqlite3` and async SQLite drivers can both implement it. */
export interface SqliteClient {
  readonly exec: (sql: string) => void | Promise<void>
  readonly run: (sql: string, params?: readonly SqliteParam[]) => SqliteRunResult | Promise<SqliteRunResult>
  readonly query: <Row extends Record<string, unknown> = Record<string, unknown>>(
    sql: string,
    params?: readonly SqliteParam[],
  ) => readonly Row[] | Promise<readonly Row[]>
  readonly transaction?: <A>(work: () => A | Promise<A>) => A | Promise<A>
}

/** Scalar used to build a stable SQLite keyset ordering blob. */
export type SqliteOrderKey = string | number | bigint | boolean

/** Backend-neutral model codec shared by SQLite adapters. */
export interface SqliteModel<T> {
  readonly table: string
  readonly id: (value: T) => RootId
  readonly order?: (value: T) => SqliteOrderKey
  readonly encode: (value: T) => unknown
  readonly decode: (value: unknown) => T
}

/** A binary keyset cursor; use the helpers below to persist it as text. */
export interface SqlitePageCursor {
  readonly table: string
  readonly order: SqlitePageOrder
  readonly orderKey?: Uint8Array
  readonly id: Uint8Array
}

/** A decoded SQLite page. */
export interface SqlitePage<T> {
  readonly items: readonly T[]
  readonly next?: SqlitePageCursor
}

export type SqlitePageOrder = "asc" | "desc"

const textEncoder = new TextEncoder()
const textDecoder = new TextDecoder("utf-8", { fatal: true })

const invalid = (message: string, operation = "SQLite model"): DbError =>
  new DbError({ kind: DbErrorKind.InvalidModel, message, operation })

const quoteIdentifier = (identifier: string): string => {
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(identifier) || /^sqlite_/i.test(identifier)) {
    throw new DbError({
      kind: DbErrorKind.InvalidIdentifier,
      message: `invalid SQLite identifier: ${identifier}`,
      operation: "SQLite identifier",
    })
  }
  return `"${identifier}"`
}

const prefixed = (prefix: number, bytes: Uint8Array): Uint8Array => {
  const output = new Uint8Array(bytes.length + 1)
  output[0] = prefix
  output.set(bytes, 1)
  return output
}

const orderedInteger = (value: bigint): Uint8Array => {
  const output = new Uint8Array(8)
  new DataView(output.buffer).setBigUint64(0, value ^ (1n << 63n), false)
  return output
}

const orderedFloat = (value: number): Uint8Array => {
  if (!Number.isFinite(value)) {
    throw invalid("SQLite ordering numbers must be finite", "SQLite order key")
  }
  const bits = new DataView(new ArrayBuffer(8))
  bits.setFloat64(0, value, false)
  const raw = bits.getBigUint64(0, false)
  const ordered = raw & (1n << 63n) ? ~raw : raw ^ (1n << 63n)
  const output = new Uint8Array(8)
  new DataView(output.buffer).setBigUint64(0, ordered, false)
  return output
}

const idBytes = (id: RootId): Uint8Array => {
  if (typeof id === "string") return prefixed(0x01, textEncoder.encode(id))
  if (typeof id === "bigint") return prefixed(0x02, orderedInteger(id))
  return prefixed(0x02, orderedInteger(BigInt(id)))
}

const orderBytes = (value: SqliteOrderKey): Uint8Array => {
  if (typeof value === "string") return prefixed(0x10, textEncoder.encode(value))
  if (typeof value === "boolean") return prefixed(0x13, new Uint8Array([value ? 1 : 0]))
  if (typeof value === "bigint") return prefixed(0x11, orderedInteger(value))
  if (Number.isInteger(value)) {
    if (!Number.isSafeInteger(value)) throw invalid("SQLite integer ordering keys must be safe integers", "SQLite order key")
    return prefixed(0x11, orderedInteger(BigInt(value)))
  }
  return prefixed(0x12, orderedFloat(value))
}

const jsonReplacer = (_key: string, value: unknown): unknown =>
  typeof value === "bigint" ? { "$appdb-bigint": value.toString(10) } : value

const jsonReviver = (_key: string, value: unknown): unknown => {
  if (
    typeof value === "object" && value !== null &&
    "$appdb-bigint" in value &&
    typeof (value as { "$appdb-bigint"?: unknown })["$appdb-bigint"] === "string"
  ) {
    return BigInt((value as { "$appdb-bigint": string })["$appdb-bigint"])
  }
  return value
}

const stringify = (value: unknown): string => JSON.stringify(value, jsonReplacer)

const parse = (value: unknown): unknown => {
  if (typeof value === "string") return JSON.parse(value, jsonReviver)
  if (value instanceof Uint8Array) return JSON.parse(textDecoder.decode(value), jsonReviver)
  throw new TypeError("SQLite payload must be TEXT or a UTF-8 byte array")
}

const toBytes = (value: unknown, label: string): Uint8Array => {
  if (value instanceof Uint8Array) return value
  throw new DbError({ kind: DbErrorKind.Engine, message: `${label} is not a SQLite BLOB`, operation: "SQLite row" })
}

const encodeCursorPayload = (cursor: SqlitePageCursor): string =>
  JSON.stringify({
    version: 1,
    table: cursor.table,
    order: cursor.order,
    orderKey: cursor.orderKey === undefined ? null : Array.from(cursor.orderKey),
    id: Array.from(cursor.id),
  })

/** Converts a binary SQLite cursor to a stable base64url token. */
export const encodeSqliteCursor = (cursor: SqlitePageCursor): string =>
  Base64Url.encode(textEncoder.encode(encodeCursorPayload(cursor)))

/** Decodes a cursor token and rejects malformed or future cursor versions. */
export const decodeSqliteCursor = (token: string): SqlitePageCursor => {
  const decoded = Base64Url.decode(token)
  if (Result.isFailure(decoded)) {
    throw new DbError({ kind: DbErrorKind.Decode, message: "SQLite cursor is not valid base64url", operation: "SQLite cursor" })
  }
  let payload: unknown
  try {
    payload = JSON.parse(textDecoder.decode(decoded.success))
  } catch (cause) {
    throw new DbError({ kind: DbErrorKind.Decode, message: "SQLite cursor JSON is malformed", operation: "SQLite cursor", cause })
  }
  if (typeof payload !== "object" || payload === null) throw invalid("SQLite cursor payload is not an object", "SQLite cursor")
  const candidate = payload as { version?: unknown; table?: unknown; order?: unknown; orderKey?: unknown; id?: unknown }
  if (candidate.version !== 1 || typeof candidate.table !== "string" || (candidate.order !== "asc" && candidate.order !== "desc") || !Array.isArray(candidate.id) || (candidate.orderKey !== null && !Array.isArray(candidate.orderKey))) {
    throw invalid("unsupported SQLite cursor version or shape", "SQLite cursor")
  }
  const bytes = (value: unknown, label: string): Uint8Array => {
    if (!Array.isArray(value) || value.some((entry) => !Number.isInteger(entry) || entry < 0 || entry > 255)) {
      throw invalid(`${label} is not a byte array`, "SQLite cursor")
    }
    return new Uint8Array(value)
  }
  return {
    table: candidate.table,
    order: candidate.order,
    ...(candidate.orderKey === null ? {} : { orderKey: bytes(candidate.orderKey, "SQLite cursor order key") }),
    id: bytes(candidate.id, "SQLite cursor id"),
  }
}

/** Adapts a plain appdb model to SQLite using its compiled schema codec. */
export const sqliteModel = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  options: { readonly orderField?: string } = {},
): SqliteModel<C["Type"]> => {
  if (model.source.kind !== "store") {
    throw invalid("SQLite storage requires a table-backed store model", "SQLite model")
  }
  const unsupported = model.fields.find(({ metadata }) =>
    metadata.foreign !== undefined || metadata.relate !== undefined || metadata.sensitive !== undefined,
  )
  if (unsupported !== undefined) {
    throw invalid("SQLite storage requires flat fields; foreign, relation, and sensitive fields need an explicit codec", "SQLite model")
  }
  const idField = model.idField
  if (idField === undefined) throw invalid(`model \`${model.table}\` must declare an id field`, "SQLite model")
  const orderField = options.orderField ?? model.paginationField
  if (orderField !== undefined && !model.fieldNames.includes(orderField)) {
    throw invalid(`SQLite order field is not declared: ${orderField}`, "SQLite model")
  }
  quoteIdentifier(model.table)
  const property = (value: C["Type"], field: string): unknown => {
    if (typeof value !== "object" || value === null || !(field in value)) {
      throw invalid(`SQLite model value is missing field \`${field}\``, "SQLite model")
    }
    return (value as Record<string, unknown>)[field]
  }
  return {
    table: model.table,
    id: (value) => assertRootId(property(value, idField), "SQLite model id"),
    ...(orderField === undefined ? {} : {
      order: (value: C["Type"]) => {
        const order = property(value, orderField)
        if (typeof order !== "string" && typeof order !== "number" && typeof order !== "bigint" && typeof order !== "boolean") {
          throw invalid(`SQLite order field \`${orderField}\` must be scalar`, "SQLite model")
        }
        return order
      },
    }),
    encode: (value) => (Schema.encodeSync as unknown as (schema: C) => (value: C["Type"]) => unknown)(model.schema)(value),
    decode: (value) => (Schema.decodeUnknownSync as unknown as (schema: C) => (value: unknown) => C["Type"])(model.schema)(value),
  }
}

interface EncodedRow {
  readonly id: Uint8Array
  readonly orderKey: Uint8Array | null
  readonly payload: string
}

const asDbError = (operation: string, cause: unknown): DbError => {
  if (cause instanceof DbError) return cause
  if (/UNIQUE constraint failed/i.test(errorMessage(cause))) {
    return new DbError({ kind: DbErrorKind.Conflict, message: `${operation} failed: ${errorMessage(cause)}`, operation, cause })
  }
  return dbErrorFromCause(operation, cause)
}

const attempt = <A>(operation: string, work: () => A | Promise<A>): Effect.Effect<A, DbError> =>
  Effect.tryPromise({ try: () => Promise.resolve().then(work), catch: (cause) => asDbError(operation, cause) })

/** Effect-native SQLite repository. It mirrors appdb's model codec boundary. */
export interface SqliteStore<T> {
  readonly init: Effect.Effect<void, DbError>
  readonly create: (value: T) => Effect.Effect<T, DbError>
  readonly save: (value: T) => Effect.Effect<T, DbError>
  readonly saveMany: (values: readonly T[]) => Effect.Effect<readonly T[], DbError>
  readonly update: (value: T) => Effect.Effect<T, DbError>
  readonly get: (id: RootId) => Effect.Effect<T | undefined, DbError>
  readonly exists: Effect.Effect<boolean, DbError>
  readonly delete: (id: RootId) => Effect.Effect<boolean, DbError>
  readonly deleteAll: Effect.Effect<void, DbError>
  readonly list: Effect.Effect<readonly T[], DbError>
  readonly page: (count: number, cursor?: SqlitePageCursor, order?: SqlitePageOrder) => Effect.Effect<SqlitePage<T>, DbError>
  readonly paginAsc: (count: number, cursor?: SqlitePageCursor) => Effect.Effect<SqlitePage<T>, DbError>
  readonly paginDesc: (count: number, cursor?: SqlitePageCursor) => Effect.Effect<SqlitePage<T>, DbError>
}

/** Creates a compact SQLite repository over any appdb-compatible codec. */
export const makeSqliteStore = <T>(client: SqliteClient, model: SqliteModel<T>): SqliteStore<T> => {
  const table = quoteIdentifier(model.table)
  const index = quoteIdentifier(`${model.table}__order_id`)
  const indexSql = model.order === undefined ? "" : ` CREATE INDEX IF NOT EXISTS ${index} ON ${table}(order_key, id);`
  const schemaSql = `CREATE TABLE IF NOT EXISTS ${table} (id BLOB NOT NULL PRIMARY KEY, order_key BLOB, payload TEXT NOT NULL) WITHOUT ROWID;${indexSql}`
  const encodeRow = (value: T): EncodedRow => {
    const id = idBytes(model.id(value))
    const orderKey = model.order === undefined ? null : orderBytes(model.order(value))
    const payload = stringify(model.encode(value))
    if (payload === undefined) throw invalid("SQLite codec produced undefined JSON", "SQLite encode")
    return { id, orderKey, payload }
  }
  const decodeRow = (row: Record<string, unknown>): T => model.decode(parse(row.payload))
  const runRow = (sql: string, row: EncodedRow): Effect.Effect<SqliteRunResult, DbError> =>
    attempt("SQLite write", () => client.run(sql, [row.id, row.orderKey, row.payload]))

  const init = attempt("SQLite schema", () => client.exec(schemaSql)).pipe(Effect.asVoid)
  const create = (value: T): Effect.Effect<T, DbError> => {
    const row = attempt("SQLite encode", () => encodeRow(value))
    return Effect.flatMap(row, (encoded) => runRow(`INSERT INTO ${table} (id, order_key, payload) VALUES (?, ?, ?)`, encoded).pipe(Effect.as(value)))
  }
  const save = (value: T): Effect.Effect<T, DbError> => {
    const row = attempt("SQLite encode", () => encodeRow(value))
    return Effect.flatMap(row, (encoded) => runRow(`INSERT INTO ${table} (id, order_key, payload) VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET order_key = excluded.order_key, payload = excluded.payload`, encoded).pipe(Effect.as(value)))
  }
  const saveMany = (values: readonly T[]): Effect.Effect<readonly T[], DbError> =>
    attempt("SQLite batch encode", () => values.map(encodeRow)).pipe(
      Effect.flatMap((rows) => attempt("SQLite batch write", async () => {
        const work = async (): Promise<void> => {
          for (const row of rows) {
            await Promise.resolve(client.run(`INSERT INTO ${table} (id, order_key, payload) VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET order_key = excluded.order_key, payload = excluded.payload`, [row.id, row.orderKey, row.payload]))
          }
        }
        if (client.transaction !== undefined) await Promise.resolve(client.transaction(work))
        else await work()
        return undefined
      })),
      Effect.as(values),
    )
  const update = (value: T): Effect.Effect<T, DbError> => {
    const row = attempt("SQLite encode", () => encodeRow(value))
    return Effect.flatMap(row, (encoded) => Effect.flatMap(
      attempt("SQLite update", () => client.run(`UPDATE ${table} SET order_key = ?, payload = ? WHERE id = ?`, [encoded.orderKey, encoded.payload, encoded.id])),
      (result) => result.changes === 0
        ? Effect.fail(new DbError({ kind: DbErrorKind.NotFound, message: "SQLite update target was not found", operation: "SQLite update" }))
        : Effect.succeed(value),
    ))
  }
  const get = (id: RootId): Effect.Effect<T | undefined, DbError> =>
    attempt("SQLite get", () => client.query(`SELECT payload FROM ${table} WHERE id = ? LIMIT 1`, [idBytes(id)])).pipe(
      Effect.map((rows) => rows[0] === undefined ? undefined : decodeRow(rows[0])),
    )
  const exists = attempt("SQLite exists", () => client.query(`SELECT 1 AS present FROM ${table} LIMIT 1`)).pipe(Effect.map((rows) => rows.length > 0))
  const remove = (id: RootId): Effect.Effect<boolean, DbError> =>
    attempt("SQLite delete", () => client.run(`DELETE FROM ${table} WHERE id = ?`, [idBytes(id)])).pipe(Effect.map((result) => result.changes > 0))
  const deleteAll = attempt("SQLite delete all", () => client.run(`DELETE FROM ${table}`)).pipe(Effect.asVoid)
  const list = attempt("SQLite list", () => client.query(`SELECT payload FROM ${table} ORDER BY ${model.order === undefined ? "id" : "order_key, id"} ASC`)).pipe(
    Effect.map((rows) => rows.map(decodeRow)),
  )
  const page = (count: number, cursor?: SqlitePageCursor, order: SqlitePageOrder = "asc"): Effect.Effect<SqlitePage<T>, DbError> => {
    if (!Number.isSafeInteger(count) || count <= 0 || count === Number.MAX_SAFE_INTEGER) {
      return Effect.fail(invalid("SQLite page count must be a positive safe integer", "SQLite page"))
    }
    const direction = order === "asc" ? "ASC" : order === "desc" ? "DESC" : (() => { throw invalid("SQLite page order must be asc or desc", "SQLite page") })()
    const ordered = model.order !== undefined
    if (cursor !== undefined && (cursor.table !== model.table || cursor.order !== order)) {
      return Effect.fail(invalid("SQLite page cursor does not match the model or direction", "SQLite page"))
    }
    const params: SqliteParam[] = []
    let where = ""
    if (ordered) {
      if (cursor !== undefined) {
        if (cursor.orderKey === undefined) return Effect.fail(invalid("ordered SQLite cursor has no order key", "SQLite page"))
        params.push(cursor.orderKey, cursor.orderKey, cursor.id)
        where = order === "asc" ? "WHERE (order_key > ? OR (order_key = ? AND id > ?))" : "WHERE (order_key < ? OR (order_key = ? AND id < ?))"
      }
    } else if (cursor !== undefined) {
      params.push(cursor.id)
      where = order === "asc" ? "WHERE id > ?" : "WHERE id < ?"
    }
    params.push(count + 1)
    const sql = ordered
      ? `SELECT id, order_key, payload FROM ${table} ${where} ORDER BY order_key ${direction}, id ${direction} LIMIT ?`
      : `SELECT id, order_key, payload FROM ${table} ${where} ORDER BY id ${direction} LIMIT ?`
    return attempt("SQLite page", () => client.query(sql, params)).pipe(
      Effect.flatMap((rows) => {
        const hasNext = rows.length > count
        const visible = hasNext ? rows.slice(0, count) : rows
        const last = visible[visible.length - 1]
        const next = hasNext && last !== undefined ? {
          table: model.table,
          order,
          ...(model.order === undefined ? {} : { orderKey: toBytes(last.order_key, "SQLite page order key") }),
          id: toBytes(last.id, "SQLite page id"),
        } : undefined
        return Effect.try({
          try: () => ({ items: visible.map(decodeRow), ...(next === undefined ? {} : { next }) }),
          catch: (cause) => asDbError("SQLite page decode", cause),
        })
      }),
    )
  }
  return {
    init,
    create,
    save,
    saveMany,
    update,
    get,
    exists,
    delete: remove,
    deleteAll,
    list,
    page,
    paginAsc: (count, cursor) => page(count, cursor, "asc"),
    paginDesc: (count, cursor) => page(count, cursor, "desc"),
  }
}
