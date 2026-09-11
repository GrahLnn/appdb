import { BoundQuery, RecordId, Table } from "surrealdb"

import { DbError } from "./errors.js"
import { rawSql, validateIdentifier } from "./query.js"
import type { RawSql } from "./query.js"

/** The minimal un-ordered edge shape shared by graph and repository writes. */
export interface RelationEdge {
  readonly in: RecordId
  readonly out: RecordId
}

/** An edge carrying the stable position used by ordered relation fields. */
export interface OrderedRelationEdge extends RelationEdge {
  readonly position: number
}

export const assertRecordId = (value: unknown, label: string): RecordId => {
  if (!(value instanceof RecordId)) {
    throw new DbError({ kind: "InvalidModel", message: `${label} must remain a full RecordId` })
  }
  return value
}

const relationTable = (relation: string): Table => {
  validateIdentifier(relation, "relation name")
  return new Table(relation)
}

const targetTable = (table: string, label: string): string => validateIdentifier(table, label)

const edgeStatement = (
  sql: string,
  relation: string,
  bindings: Record<string, unknown>,
): RawSql => rawSql(sql, { rel: relationTable(relation), ...bindings })

export const relateStatement = (inId: RecordId, outId: RecordId, relation: string): RawSql =>
  edgeStatement(
    "INSERT RELATION INTO $rel [{ in: $in, out: $out, created_at: time::now() }] RETURN NONE;",
    relation,
    { in: assertRecordId(inId, "relation source"), out: assertRecordId(outId, "relation target") },
  )

export const backRelateStatement = (selfId: RecordId, targetId: RecordId, relation: string): RawSql =>
  relateStatement(targetId, selfId, relation)

export const unrelateStatement = (selfId: RecordId, targetId: RecordId, relation: string): RawSql =>
  edgeStatement(
    "DELETE $rel WHERE in = $in AND out = $out RETURN NONE;",
    relation,
    { in: assertRecordId(selfId, "relation source"), out: assertRecordId(targetId, "relation target") },
  )

export const unrelateAllStatement = (selfId: RecordId, relation: string): RawSql =>
  edgeStatement(
    "DELETE $rel WHERE in = $in RETURN NONE;",
    relation,
    { in: assertRecordId(selfId, "relation source") },
  )

export const outgoingIdsStatement = (
  inId: RecordId,
  relation: string,
  outTable?: string,
): RawSql => {
  const source = assertRecordId(inId, "relation source")
  if (outTable === undefined) {
    return edgeStatement("RETURN (SELECT VALUE out FROM $rel WHERE in = $in);", relation, { in: source })
  }
  return edgeStatement(
    "RETURN (SELECT VALUE out FROM $rel WHERE in = $in AND record::tb(out) = $out_table);",
    relation,
    { in: source, out_table: targetTable(outTable, "out table") },
  )
}

export const incomingIdsStatement = (
  outId: RecordId,
  relation: string,
  inTable?: string,
): RawSql => {
  const target = assertRecordId(outId, "relation target")
  if (inTable === undefined) {
    return edgeStatement("RETURN (SELECT VALUE in FROM $rel WHERE out = $out);", relation, { out: target })
  }
  return edgeStatement(
    "RETURN (SELECT VALUE in FROM $rel WHERE out = $out AND record::tb(in) = $in_table);",
    relation,
    { out: target, in_table: targetTable(inTable, "in table") },
  )
}

export const outgoingRowsStatement = (
  inId: RecordId,
  relation: string,
  outTable?: string,
): RawSql => {
  const source = assertRecordId(inId, "relation source")
  const filter = outTable === undefined ? "" : " AND record::tb(out) = $out_table"
  const bindings = outTable === undefined
    ? { in: source }
    : { in: source, out_table: targetTable(outTable, "out table") }
  return edgeStatement(
    `LET $ids = (SELECT VALUE out FROM $rel WHERE in = $in${filter}); SELECT *, record::id(id) AS id FROM $ids;`,
    relation,
    bindings,
  )
}

export const incomingRowsStatement = (
  outId: RecordId,
  relation: string,
  inTable?: string,
): RawSql => {
  const target = assertRecordId(outId, "relation target")
  const filter = inTable === undefined ? "" : " AND record::tb(in) = $in_table"
  const bindings = inTable === undefined
    ? { out: target }
    : { out: target, in_table: targetTable(inTable, "in table") }
  return edgeStatement(
    `LET $ids = (SELECT VALUE in FROM $rel WHERE out = $out${filter}); SELECT *, record::id(id) AS id FROM $ids;`,
    relation,
    bindings,
  )
}

/** Reads ordered outgoing edges for several owners in one result slot. */
export const outgoingRowsForOwnersStatement = (
  inIds: readonly RecordId[],
  relation: string,
): RawSql =>
  edgeStatement(
    "SELECT `in` AS owner, out, position FROM $rel WHERE `in` IN $owners ORDER BY `in` ASC, position ASC;",
    relation,
    { owners: inIds.map((id) => assertRecordId(id, "relation source")) },
  )

/** Reads ordered incoming edges for several owners in one result slot. */
export const incomingRowsForOwnersStatement = (
  outIds: readonly RecordId[],
  relation: string,
): RawSql =>
  edgeStatement(
    "SELECT out AS owner, `in`, position FROM $rel WHERE out IN $owners ORDER BY out ASC, position ASC;",
    relation,
    { owners: outIds.map((id) => assertRecordId(id, "relation target")) },
  )

export const outgoingCountStatement = (
  inId: RecordId,
  relation: string,
  outTable?: string,
): RawSql => {
  const source = assertRecordId(inId, "relation source")
  const filter = outTable === undefined ? "" : " AND record::tb(out) = $out_table"
  const bindings = outTable === undefined
    ? { in: source }
    : { in: source, out_table: targetTable(outTable, "out table") }
  return edgeStatement(
    `RETURN count((SELECT VALUE out FROM $rel WHERE in = $in${filter}));`,
    relation,
    bindings,
  )
}

export const incomingCountStatement = (
  outId: RecordId,
  relation: string,
  inTable?: string,
): RawSql => {
  const target = assertRecordId(outId, "relation target")
  const filter = inTable === undefined ? "" : " AND record::tb(in) = $in_table"
  const bindings = inTable === undefined
    ? { out: target }
    : { out: target, in_table: targetTable(inTable, "in table") }
  return edgeStatement(
    `RETURN count((SELECT VALUE in FROM $rel WHERE out = $out${filter}));`,
    relation,
    bindings,
  )
}

export const outgoingEdgesStatement = (inId: RecordId, relation: string): RawSql =>
  edgeStatement(
    "SELECT `in` AS source, out, position FROM $rel WHERE in = $in ORDER BY position ASC;",
    relation,
    { in: assertRecordId(inId, "relation source") },
  )

export const incomingEdgesStatement = (outId: RecordId, relation: string): RawSql =>
  edgeStatement(
    "SELECT `in` AS source, out, position FROM $rel WHERE out = $out ORDER BY position ASC;",
    relation,
    { out: assertRecordId(outId, "relation target") },
  )

/** Adds the ordered edge statements used by repository atomic writes. */
export const appendOrderedRelationEdges = (
  statement: RawSql,
  relation: string,
  edges: readonly OrderedRelationEdge[],
  prefix: string,
): { readonly statement: RawSql; readonly statementCount: number } => {
  validateIdentifier(relation, "relation name")
  const next = new BoundQuery(statement)
  const relationSql = new Table(relation).toString()
  for (const [index, edge] of edges.entries()) {
    const inId = assertRecordId(edge.in, "relation source")
    const outId = assertRecordId(edge.out, "relation target")
    if (!Number.isSafeInteger(edge.position) || edge.position < 0) {
      throw new DbError({ kind: "InvalidModel", message: "relation edge position must be a non-negative safe integer" })
    }
    next.append(
      `RELATE $${prefix}_in_${index} -> ${relationSql} -> $${prefix}_out_${index} SET position = $${prefix}_position_${index};`,
      {
        [`${prefix}_in_${index}`]: inId,
        [`${prefix}_out_${index}`]: outId,
        [`${prefix}_position_${index}`]: edge.position,
      },
    )
  }
  return { statement: next, statementCount: edges.length }
}
