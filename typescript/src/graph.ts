import { Effect, Schema } from "effect"
import { RecordId } from "surrealdb"

import { Database } from "./connection.js"
import { DbError, DbErrorKind, isDbError } from "./errors.js"
import type { Model } from "./model.js"
import { hydrateRows, type RepositoryError } from "./repository.js"
import {
  assertRecordId,
  backRelateStatement,
  incomingCountStatement,
  incomingEdgesStatement,
  incomingIdsStatement,
  incomingRowsStatement,
  outgoingCountStatement,
  outgoingEdgesStatement,
  outgoingIdsStatement,
  outgoingRowsStatement,
  relateStatement,
  unrelateAllStatement,
  unrelateStatement,
  type OrderedRelationEdge,
  type RelationEdge,
} from "./relation.js"
import { query, type RawSql } from "./query.js"

export type GraphCount = number | bigint

const wrapBuildError = (cause: unknown): DbError => {
  if (isDbError(cause)) return cause
  return new DbError({
    kind: DbErrorKind.InvalidModel,
    message: cause instanceof Error ? cause.message : String(cause),
    operation: "graph",
    cause,
  })
}

const execute = <A>(build: () => RawSql, read: (slots: readonly unknown[]) => A): Effect.Effect<A, DbError, Database> =>
  Effect.try({ try: build, catch: wrapBuildError }).pipe(
    Effect.flatMap((statement) => query(statement)),
    Effect.flatMap((slots) =>
      Effect.try({ try: () => read(slots), catch: wrapBuildError }),
    ),
  )

const slot = (slots: readonly unknown[], index: number, operation: string): unknown => {
  if (index < 0 || index >= slots.length) {
    throw new DbError({ kind: DbErrorKind.EmptyResult, message: `${operation} result slot ${index} is missing`, operation })
  }
  return slots[index]
}

const recordIds = (value: unknown, operation: string): readonly RecordId[] => {
  if (!Array.isArray(value)) {
    throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} did not return a record-id array`, operation })
  }
  return value.map((item) => assertRecordId(item, `${operation} record id`))
}

const rows = (slots: readonly unknown[], index: number, operation: string): readonly unknown[] => {
  const value = slot(slots, index, operation)
  if (!Array.isArray(value)) {
    throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} did not return a row array`, operation })
  }
  return value
}

const countValue = (value: unknown, operation: string): GraphCount => {
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} returned an invalid count`, operation })
    }
    return value
  }
  if (typeof value === "bigint") {
    if (value < 0n) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} returned an invalid count`, operation })
    }
    return value
  }
  throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} did not return a numeric count`, operation })
}

const decodeRows = <Table extends string, C extends Schema.Top>(
  model: Model<Table, C>,
  values: readonly unknown[],
): Effect.Effect<readonly C["Type"][], RepositoryError, Database | C["DecodingServices"]> =>
  hydrateRows(model, values)

const orderedEdges = (slots: readonly unknown[], operation: string): readonly OrderedRelationEdge[] => {
  const value = rows(slots, 0, operation)
  return value.map((item) => {
    if (typeof item !== "object" || item === null) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} returned a malformed edge`, operation })
    }
    const row = item as Record<string, unknown>
    const source = assertRecordId(row.source ?? row.in, `${operation} source`)
    const out = assertRecordId(row.out, `${operation} target`)
    const position = row.position
    if (typeof position !== "number" || !Number.isSafeInteger(position) || position < 0) {
      throw new DbError({ kind: DbErrorKind.Decode, message: `${operation} returned an invalid edge position`, operation })
    }
    return { in: source, out, position }
  })
}

/** Effect-native graph operations over the shared Database service. */
export class GraphRepo {
  static relateAt(inId: RecordId, outId: RecordId, relation: string): Effect.Effect<void, DbError, Database> {
    return execute(
      () => relateStatement(inId, outId, relation),
      () => undefined,
    )
  }

  static backRelateAt(selfId: RecordId, targetId: RecordId, relation: string): Effect.Effect<void, DbError, Database> {
    return execute(
      () => backRelateStatement(selfId, targetId, relation),
      () => undefined,
    )
  }

  static unrelateAt(selfId: RecordId, targetId: RecordId, relation: string): Effect.Effect<void, DbError, Database> {
    return execute(
      () => unrelateStatement(selfId, targetId, relation),
      () => undefined,
    )
  }

  static unrelateAll(selfId: RecordId, relation: string): Effect.Effect<void, DbError, Database> {
    return execute(
      () => unrelateAllStatement(selfId, relation),
      () => undefined,
    )
  }

  static outgoingIds(
    inId: RecordId,
    relation: string,
    outTable?: string,
  ): Effect.Effect<readonly RecordId[], DbError, Database> {
    return execute(
      () => outgoingIdsStatement(inId, relation, outTable),
      (slots) => recordIds(slot(slots, 0, "outgoingIds"), "outgoingIds"),
    )
  }

  static outIds(
    inId: RecordId,
    relation: string,
    outTable?: string,
  ): Effect.Effect<readonly RecordId[], DbError, Database> {
    return this.outgoingIds(inId, relation, outTable)
  }

  static incomingIds(
    outId: RecordId,
    relation: string,
    inTable?: string,
  ): Effect.Effect<readonly RecordId[], DbError, Database> {
    return execute(
      () => incomingIdsStatement(outId, relation, inTable),
      (slots) => recordIds(slot(slots, 0, "incomingIds"), "incomingIds"),
    )
  }

  static inIds(
    outId: RecordId,
    relation: string,
    inTable?: string,
  ): Effect.Effect<readonly RecordId[], DbError, Database> {
    return this.incomingIds(outId, relation, inTable)
  }

  static outgoingCount(
    inId: RecordId,
    relation: string,
    outTable?: string,
  ): Effect.Effect<GraphCount, DbError, Database> {
    return execute(
      () => outgoingCountStatement(inId, relation, outTable),
      (slots) => countValue(slot(slots, 0, "outgoingCount"), "outgoingCount"),
    )
  }

  static outgoingCountAs<Table extends string, C extends Schema.Top>(
    inId: RecordId,
    relation: string,
    target: Model<Table, C>,
  ): Effect.Effect<GraphCount, DbError, Database> {
    return this.outgoingCount(inId, relation, target.table)
  }

  static incomingCount(
    outId: RecordId,
    relation: string,
    inTable?: string,
  ): Effect.Effect<GraphCount, DbError, Database> {
    return execute(
      () => incomingCountStatement(outId, relation, inTable),
      (slots) => countValue(slot(slots, 0, "incomingCount"), "incomingCount"),
    )
  }

  static incomingCountAs<Table extends string, C extends Schema.Top>(
    outId: RecordId,
    relation: string,
    target: Model<Table, C>,
  ): Effect.Effect<GraphCount, DbError, Database> {
    return this.incomingCount(outId, relation, target.table)
  }

  static outgoingEdges(inId: RecordId, relation: string): Effect.Effect<readonly OrderedRelationEdge[], DbError, Database> {
    return execute(
      () => outgoingEdgesStatement(inId, relation),
      (slots) => orderedEdges(slots, "outgoingEdges"),
    )
  }

  static outEdges(inId: RecordId, relation: string): Effect.Effect<readonly OrderedRelationEdge[], DbError, Database> {
    return this.outgoingEdges(inId, relation)
  }

  static incomingEdges(outId: RecordId, relation: string): Effect.Effect<readonly OrderedRelationEdge[], DbError, Database> {
    return execute(
      () => incomingEdgesStatement(outId, relation),
      (slots) => orderedEdges(slots, "incomingEdges"),
    )
  }

  static inEdges(outId: RecordId, relation: string): Effect.Effect<readonly OrderedRelationEdge[], DbError, Database> {
    return this.incomingEdges(outId, relation)
  }

  /** Raw outgoing rows are exposed for repository hydration and view projection owners. */
  static outgoingRows(
    inId: RecordId,
    relation: string,
    outTable?: string,
  ): Effect.Effect<readonly unknown[], DbError, Database> {
    return execute(
      () => outgoingRowsStatement(inId, relation, outTable),
      (slots) => rows(slots, 1, "outgoingRows"),
    )
  }

  /** Raw incoming rows are exposed for repository hydration and view projection owners. */
  static incomingRows(
    outId: RecordId,
    relation: string,
    inTable?: string,
  ): Effect.Effect<readonly unknown[], DbError, Database> {
    return execute(
      () => incomingRowsStatement(outId, relation, inTable),
      (slots) => rows(slots, 1, "incomingRows"),
    )
  }

  /**
   * Hydrate rows through the repository's public seam. This preserves the
   * foreign, secure, and relation-bearing model path instead of decoding a
   * graph projection as a plain schema object.
   */
  static outgoing<Table extends string, C extends Schema.Top>(
    inId: RecordId,
    relation: string,
    target: Model<Table, C>,
  ): Effect.Effect<readonly C["Type"][], RepositoryError, Database | C["DecodingServices"]> {
    return this.outgoingRows(inId, relation, target.table).pipe(
      Effect.flatMap((values) => decodeRows(target, values)),
    )
  }

  static incoming<Table extends string, C extends Schema.Top>(
    outId: RecordId,
    relation: string,
    target: Model<Table, C>,
  ): Effect.Effect<readonly C["Type"][], RepositoryError, Database | C["DecodingServices"]> {
    return this.incomingRows(outId, relation, target.table).pipe(
      Effect.flatMap((values) => decodeRows(target, values)),
    )
  }
}

export const relateAt = (inId: RecordId, outId: RecordId, relation: string) =>
  GraphRepo.relateAt(inId, outId, relation)

export const backRelateAt = (selfId: RecordId, targetId: RecordId, relation: string) =>
  GraphRepo.backRelateAt(selfId, targetId, relation)

export const unrelateAt = (selfId: RecordId, targetId: RecordId, relation: string) =>
  GraphRepo.unrelateAt(selfId, targetId, relation)

export const unrelateAll = (selfId: RecordId, relation: string) =>
  GraphRepo.unrelateAll(selfId, relation)

export const outgoingIds = (inId: RecordId, relation: string, outTable?: string) =>
  GraphRepo.outgoingIds(inId, relation, outTable)

export const outIds = (inId: RecordId, relation: string, outTable?: string) =>
  GraphRepo.outgoingIds(inId, relation, outTable)

export const incomingIds = (outId: RecordId, relation: string, inTable?: string) =>
  GraphRepo.incomingIds(outId, relation, inTable)

export const inIds = (outId: RecordId, relation: string, inTable?: string) =>
  GraphRepo.incomingIds(outId, relation, inTable)

export const outgoingCount = (inId: RecordId, relation: string, outTable?: string) =>
  GraphRepo.outgoingCount(inId, relation, outTable)

export const outgoingCountAs = <Table extends string, C extends Schema.Top>(
  inId: RecordId,
  relation: string,
  target: Model<Table, C>,
) => GraphRepo.outgoingCountAs(inId, relation, target)

export const incomingCount = (outId: RecordId, relation: string, inTable?: string) =>
  GraphRepo.incomingCount(outId, relation, inTable)

export const incomingCountAs = <Table extends string, C extends Schema.Top>(
  outId: RecordId,
  relation: string,
  target: Model<Table, C>,
) => GraphRepo.incomingCountAs(outId, relation, target)

export const outgoingEdges = (inId: RecordId, relation: string) =>
  GraphRepo.outgoingEdges(inId, relation)

export const outEdges = (inId: RecordId, relation: string) =>
  GraphRepo.outgoingEdges(inId, relation)

export const incomingEdges = (outId: RecordId, relation: string) =>
  GraphRepo.incomingEdges(outId, relation)

export const inEdges = (outId: RecordId, relation: string) =>
  GraphRepo.incomingEdges(outId, relation)

export const outgoingRows = (inId: RecordId, relation: string, outTable?: string) =>
  GraphRepo.outgoingRows(inId, relation, outTable)

export const incomingRows = (outId: RecordId, relation: string, inTable?: string) =>
  GraphRepo.incomingRows(outId, relation, inTable)

export const outgoing = <Table extends string, C extends Schema.Top>(
  inId: RecordId,
  relation: string,
  target: Model<Table, C>,
) => GraphRepo.outgoing(inId, relation, target)

export const incoming = <Table extends string, C extends Schema.Top>(
  outId: RecordId,
  relation: string,
  target: Model<Table, C>,
) => GraphRepo.incoming(outId, relation, target)

export type { OrderedRelationEdge, RelationEdge }
