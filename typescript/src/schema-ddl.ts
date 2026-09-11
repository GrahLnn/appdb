import { Effect } from "effect"

import type { AnyModel } from "./model.js"
import { DbError, DbErrorKind, isDbError } from "./errors.js"
import { validateIdentifier } from "./query.js"

/** A query owner capable of executing one schema statement at a time. */
export interface SchemaExecutor<E = unknown> {
  readonly query: (statement: string) => Effect.Effect<unknown, E>
}

/** The scalar types accepted by SurrealDB's HNSW index definition. */
export type HnswVectorType =
  | "F64"
  | "F32"
  | "F16"
  | "I64"
  | "I32"
  | "I16"
  | "I8"
  | "U64"
  | "U32"
  | "U16"
  | "U8"

/** The distance metrics accepted by SurrealDB's HNSW index definition. */
export type HnswDistance =
  | "EUCLIDEAN"
  | "COSINE"
  | "INNER_PRODUCT"
  | "COSINE_NORMALIZED"

/** Explicit HNSW registration, kept separate from Effect Schema field types. */
export interface HnswIndexDefinition {
  readonly name: string
  readonly table: string
  /** A plain field name or a dotted field path, as supported by SurrealDB. */
  readonly field: string
  readonly dimension: number
  readonly vectorType?: HnswVectorType
  readonly distance?: HnswDistance
  readonly efConstruction?: number
  readonly m?: number
  readonly concurrently?: boolean
  readonly defer?: boolean
}

/** Explicit DDL plus metadata-driven index definitions for one apply call. */
export interface SchemaDdlDefinition {
  /** Raw DDL is preserved byte-for-byte and is executed before generated DDL. */
  readonly rawDdl?: readonly string[]
  /** Models whose real Store owner supplies table/index metadata. */
  readonly models?: readonly SchemaModel[]
  readonly hnsw?: readonly HnswIndexDefinition[]
}

/** A real model descriptor; source identity is retained for owner resolution. */
export type SchemaModel = AnyModel

const fieldPath = /^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$/

const invalid = (message: string): never => {
  throw new DbError({ kind: DbErrorKind.InvalidIdentifier, message, operation: "schema DDL" })
}

const invalidModel = (message: string): never => {
  throw new DbError({ kind: DbErrorKind.InvalidModel, message, operation: "schema DDL" })
}

const validateFieldPath = (value: string, label: string): string => {
  if (!fieldPath.test(value)) invalid(`${label} must be a plain SurrealQL field path: ${value}`)
  return value
}

const validatePositiveInteger = (value: number, label: string): number => {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new DbError({
      kind: DbErrorKind.InvalidModel,
      message: `${label} must be a positive safe integer`,
      operation: "schema DDL",
    })
  }
  return value
}

const validateRawDdl = (statement: string): string => {
  if (statement.trim().length === 0) {
    throw new DbError({
      kind: DbErrorKind.InvalidModel,
      message: "raw schema DDL statements must not be empty",
      operation: "schema DDL",
    })
  }
  return statement
}

/** Builds the Rust-compatible idempotent table bootstrap statement. */
export const tableBootstrapDdl = (table: string): string =>
  `DEFINE TABLE IF NOT EXISTS ${validateIdentifier(table, "schema table")} SCHEMALESS;`

const indexName = (table: string, field: string, suffix: string): string =>
  `${validateIdentifier(table, "schema table")}_${validateIdentifier(field, "schema field")}_${suffix}`

/** Builds one independent unique index, matching #[unique]. */
export const uniqueIndexDdl = (table: string, field: string): string => {
  const checkedTable = validateIdentifier(table, "schema table")
  const checkedField = validateIdentifier(field, "schema field")
  return `DEFINE INDEX IF NOT EXISTS ${indexName(checkedTable, checkedField, "unique")} ON ${checkedTable} FIELDS ${checkedField} UNIQUE;`
}

/**
 * Builds the ordinary keyset index emitted for #[pagin]. A pagination field
 * that is the model's public id alias still maps to SurrealDB's physical `id`.
 */
export const paginationIndexDdl = (
  table: string,
  field: string,
  idField = "id",
): string => {
  const checkedTable = validateIdentifier(table, "schema table")
  const checkedField = validateIdentifier(field, "schema field")
  const checkedIdField = validateIdentifier(idField, "schema id field")
  const physicalField = checkedField === checkedIdField ? "id" : checkedField
  const indexFields = physicalField === "id" ? "id" : `${physicalField},id`
  return `DEFINE INDEX IF NOT EXISTS ${indexName(checkedTable, checkedField, "id_pagin")} ON ${checkedTable} FIELDS ${indexFields};`
}

/** Builds the explicit HNSW DDL without rewriting caller-supplied SQL. */
export const hnswIndexDdl = (definition: HnswIndexDefinition): string => {
  const name = validateIdentifier(definition.name, "HNSW index name")
  const table = validateIdentifier(definition.table, "HNSW table")
  const field = validateFieldPath(definition.field, "HNSW field")
  const dimension = validatePositiveInteger(definition.dimension, "HNSW dimension")
  const efConstruction = definition.efConstruction === undefined
    ? undefined
    : validatePositiveInteger(definition.efConstruction, "HNSW efConstruction")
  const m = definition.m === undefined ? undefined : validatePositiveInteger(definition.m, "HNSW m")

  let ddl = `DEFINE INDEX IF NOT EXISTS ${name} ON ${table} FIELDS ${field} HNSW DIMENSION ${dimension}`
  if (definition.vectorType !== undefined) ddl += ` TYPE ${definition.vectorType}`
  if (definition.distance !== undefined) ddl += ` DIST ${definition.distance}`
  if (efConstruction !== undefined) ddl += ` EFC ${efConstruction}`
  if (m !== undefined) ddl += ` M ${m}`
  if (definition.concurrently === true) ddl += " CONCURRENTLY"
  if (definition.defer === true) ddl += " DEFER"
  return `${ddl};`
}

/**
 * Derives the generated statements for one model. Raw DDL remains a separate
 * input so arbitrary schemafull/field statements are never inferred from an
 * Effect AST or silently rewritten.
 */
const storeOwner = (model: AnyModel): AnyModel => {
  let current = model
  const seen = new Set<AnyModel>()
  while (current.source.kind === "view") {
    if (seen.has(current)) invalidModel("cyclic table-view owner chain cannot define schema")
    seen.add(current)
    current = current.source.owner
  }
  if (current.source.kind !== "store") {
    invalidModel(`SQL view \`${current.table}\` cannot define table or index DDL`)
  }
  return current
}

export const modelSchemaDdl = (model: AnyModel): readonly string[] => {
  const owner = storeOwner(model)
  const output = [tableBootstrapDdl(owner.table)]
  for (const field of owner.uniqueFields) output.push(uniqueIndexDdl(owner.table, field))
  if (owner.paginationField !== undefined) {
    output.push(paginationIndexDdl(owner.table, owner.paginationField, owner.idField ?? "id"))
  }
  return output
}

/** Returns raw-first, then generated table/index/HNSW statements in order. */
export const schemaDdl = (definition: SchemaDdlDefinition): readonly string[] => {
  const raw = (definition.rawDdl ?? []).map(validateRawDdl)
  const generated = [...new Set([
    ...(definition.models ?? []).flatMap(modelSchemaDdl),
    ...(definition.hnsw ?? []).map(hnswIndexDdl),
  ])]
  // Raw statements may intentionally repeat or contain DML. Preserve their
  // count and bytes; only generated IF NOT EXISTS statements are deduplicated.
  return [...raw, ...generated]
}

/**
 * Applies statements sequentially. The executor decides how startup errors
 * prevent publication of its ready service; this function stops at the first
 * failed statement and never rewrites raw SQL.
 */
export const applySchema = <E>(
  executor: SchemaExecutor<E>,
  definition: SchemaDdlDefinition,
): Effect.Effect<void, E | DbError> =>
  Effect.flatMap(
    Effect.try({
      try: () => schemaDdl(definition),
      catch: (cause) => isDbError(cause)
        ? cause
        : new DbError({
            kind: DbErrorKind.InvalidModel,
            message: `schema DDL validation failed: ${String(cause)}`,
            operation: "schema DDL",
            cause,
          }),
    }),
    (statements) => Effect.forEach(statements, (statement) => executor.query(statement), {
      concurrency: 1,
      discard: true,
    }),
  )
