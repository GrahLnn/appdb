import { Data } from "effect"

/** Stable error categories shared by the TypeScript runtime owners. */
export const DbErrorKind = {
  Transport: "Transport",
  Engine: "Engine",
  NotFound: "NotFound",
  MissingTable: "MissingTable",
  Conflict: "Conflict",
  Decode: "Decode",
  EmptyResult: "EmptyResult",
  InvalidIdentifier: "InvalidIdentifier",
  InvalidModel: "InvalidModel",
  NotInitialized: "NotInitialized",
  AlreadyInitialized: "AlreadyInitialized",
} as const

export type DbErrorKind = (typeof DbErrorKind)[keyof typeof DbErrorKind]

/** A typed boundary error understood by repository, query, and model owners. */
export class DbError extends Data.TaggedError("DbError")<{
  readonly kind: DbErrorKind
  readonly message: string
  readonly operation?: string
  readonly cause?: unknown
}> {}

export interface DbErrorOptions {
  readonly operation?: string
  readonly cause?: unknown
}

const withOptions = (
  kind: DbErrorKind,
  message: string,
  options: DbErrorOptions,
): ConstructorParameters<typeof DbError>[0] => {
  const fields: {
    kind: DbErrorKind
    message: string
    operation?: string
    cause?: unknown
  } = { kind, message }
  if (options.operation !== undefined) {
    fields.operation = options.operation
  }
  if (options.cause !== undefined) {
    fields.cause = options.cause
  }
  return fields
}

export const makeDbError = (
  kind: DbErrorKind,
  message: string,
  options: DbErrorOptions = {},
): DbError => new DbError(withOptions(kind, message, options))

export const isDbError = (value: unknown): value is DbError =>
  typeof value === "object" && value !== null && "_tag" in value && value._tag === "DbError"

export const errorMessage = (cause: unknown): string => {
  if (cause instanceof Error) return cause.message
  if (typeof cause === "object" && cause !== null && "message" in cause && typeof cause.message === "string") {
    return cause.message
  }
  return String(cause)
}

type ErrorShape = {
  readonly name?: unknown
  readonly kind?: unknown
  readonly message?: unknown
  readonly details?: unknown
  readonly cause?: unknown
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null

const errorShape = (value: unknown): ErrorShape | undefined =>
  isObject(value) ? value : undefined

const stringField = (value: unknown): string | undefined =>
  typeof value === "string" ? value : undefined

const detailKind = (value: unknown): string | undefined => {
  const details = errorShape(value)?.details
  return stringField(errorShape(details)?.kind)
}

const errorChain = function* (cause: unknown): Generator<ErrorShape> {
  const seen = new Set<object>()
  let current: unknown = cause
  while (isObject(current) && !seen.has(current)) {
    seen.add(current)
    const shape = errorShape(current)
    if (shape === undefined) return
    yield shape
    current = shape.cause
  }
}

/**
 * Classify a native SDK/engine error at the shared database boundary.
 *
 * SurrealDB exposes duplicate records as `AlreadyExistsError` (kind
 * `AlreadyExists`), while older SDKs and the Rust implementation use conflict
 * wording. Keep these cases in one classifier so connection, query, and
 * transaction owners cannot drift in their mappings.
 */
export const classifyErrorKind = (
  cause: unknown,
  fallback: DbErrorKind = DbErrorKind.Engine,
): DbErrorKind => {
  if (isDbError(cause)) return cause.kind

  for (const error of errorChain(cause)) {
    const name = stringField(error.name)
    const kind = stringField(error.kind)
    const message = stringField(error.message) ?? errorMessage(error)
    const details = detailKind(error)

    if (name === "ConnectionUnavailableError" || name === "HttpConnectionError" || kind === "Connection") {
      return DbErrorKind.Transport
    }
    if (name === "NotFoundError" || kind === "NotFound") return DbErrorKind.NotFound
    if (name === "InvalidRecordIdError" || name === "InvalidTableError") return DbErrorKind.InvalidIdentifier
    if (name === "SerializationError" || kind === "Serialization") return DbErrorKind.Decode
    if (name === "AlreadyExistsError" || name === "ConflictError" || kind === "AlreadyExists" || kind === "Conflict") {
      return DbErrorKind.Conflict
    }
    if (details === "AlreadyExists" || details === "Conflict") return DbErrorKind.Conflict
    if (/table.+does not exist/i.test(message)) return DbErrorKind.MissingTable
    if (/record.+not found|not found/i.test(message)) return DbErrorKind.NotFound
    if (/already exists|duplicate key|constraint violation|\bconflict\b/i.test(message)) {
      return DbErrorKind.Conflict
    }
    if (/failed to deserialize|invalid type|missing field|unknown variant|\bexpected\b|\bdecode\b/i.test(message)) {
      return DbErrorKind.Decode
    }
    if (/transport|connection|socket|timed out/i.test(message)) return DbErrorKind.Transport
  }

  return fallback
}

/** Convert an arbitrary native error into the shared typed boundary error. */
export const dbErrorFromCause = (
  operation: string,
  cause: unknown,
  fallback: DbErrorKind = DbErrorKind.Engine,
): DbError => {
  if (isDbError(cause)) return cause
  return makeDbError(
    classifyErrorKind(cause, fallback),
    `${operation} failed: ${errorMessage(cause)}`,
    { operation, cause },
  )
}

/** True for SDK response errors emitted only because an earlier query failed. */
export const isRollbackPlaceholder = (cause: unknown): boolean => {
  for (const error of errorChain(cause)) {
    const name = stringField(error.name)
    const kind = stringField(error.kind)
    const details = detailKind(error)
    if ((name === "QueryError" || kind === "Query") && details === "NotExecuted") return true
  }
  return false
}

/** Converts a schema or codec failure into the shared Decode category. */
export const decodeError = (
  cause: unknown,
  operation?: string,
): DbError =>
  makeDbError(DbErrorKind.Decode, errorMessage(cause), {
    ...(operation === undefined ? {} : { operation }),
    cause,
  })
