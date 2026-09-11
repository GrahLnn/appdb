import { Schema } from "effect"
import { RecordId, type RecordIdValue } from "surrealdb"
import { DbError, DbErrorKind } from "./errors.js"

/** Rust's public id domain: strings, safe JS integers, and i64-compatible bigint values. */
export const I64_MIN = -(2n ** 63n)
export const I64_MAX = 2n ** 63n - 1n

export const I64Schema = Schema.BigInt.pipe(
  Schema.check(
    Schema.isBetweenBigInt({ minimum: I64_MIN, maximum: I64_MAX }),
  ),
)

/** `Schema.Int` rejects non-integers and values outside JavaScript's exact range. */
export const RootIntSchema = Schema.Int
export const RootIdSchema = Schema.Union([Schema.String, RootIntSchema, I64Schema])

export type RootId = Schema.Schema.Type<typeof RootIdSchema>
export type RootIdEncoded = Schema.Codec.Encoded<typeof RootIdSchema>
export type ForeignId<Table extends string = string> = RecordId<Table, RootId>

export const isRootId = (value: unknown): value is RootId => Schema.is(RootIdSchema)(value)

export const assertRootId = (value: unknown, operation = "decode id"): RootId => {
  if (isRootId(value)) {
    return value
  }
  throw new DbError({
    kind: DbErrorKind.InvalidIdentifier,
    message: "expected a string, safe integer, or i64-compatible bigint id",
    operation,
  })
}

export const makeRecordId = <Table extends string>(
  table: Table,
  id: RootId,
): RecordId<Table, RootId> => new RecordId(table, id)

/**
 * Converts a typed SDK record id to the public scalar key after checking its
 * table identity. The RecordId object itself remains available to foreign and
 * graph consumers; this helper is only for the public root `id` field.
 */
export const rootIdFromRecord = <Table extends string>(
  record: RecordId<Table, RootId>,
  expectedTable?: Table,
): RootId => {
  if (expectedTable !== undefined && record.table.name !== expectedTable) {
    throw new DbError({
      kind: DbErrorKind.InvalidIdentifier,
      message: `record id belongs to table ${record.table.name}, expected ${expectedTable}`,
      operation: "decode record id",
    })
  }
  return assertRootId(record.id, "decode record id key")
}

export const isRecordId = (value: unknown): value is RecordId<string, RecordIdValue> =>
  value instanceof RecordId
