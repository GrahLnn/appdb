import { Schema, SchemaAST } from "effect"
import type { KeyContext } from "./crypto.js"
import type { Model } from "./model.js"
import { DbError, DbErrorKind } from "./errors.js"

// Kept outside the public metadata shape. It lets the repository prove that a
// foreign field was encoded by the same schema instance as its lazy target
// before reusing that encoded value for the child write plan.
const FIELD_SCHEMA: unique symbol = Symbol("appdb/field-schema")
type InternalFieldMeta = FieldMeta & { readonly [FIELD_SCHEMA]?: Schema.Top }

// `any` is intentional here: Model's encode/decode functions are input
// contravariant, so a structural wildcard is needed for lazy model targets.
export type AnyModel = Model<string, any, any>
export type FieldPath = readonly PropertyKey[]
export type RelationDirection = "outgoing" | "incoming"

export interface ForeignFieldMeta<M extends AnyModel = AnyModel> {
  readonly target: () => M
}

export interface RelateFieldMeta<M extends AnyModel = AnyModel> {
  readonly target: () => M
  readonly direction: RelationDirection
  readonly relation: string
}

export interface SensitiveFieldMeta {
  /** `leaf` encrypts recursive leaves; `value` encrypts one encoded value. */
  readonly scope: "leaf" | "value"
  readonly keyContext?: KeyContext
}

/** Metadata attached to a native Effect Schema node. It carries no value type. */
export interface FieldMeta<M extends AnyModel = AnyModel> {
  readonly id?: true
  readonly unique?: true
  readonly pagination?: true
  readonly foreign?: ForeignFieldMeta<M>
  readonly relate?: RelateFieldMeta<M>
  readonly sensitive?: SensitiveFieldMeta
}

export type ReferenceCardinality = "one" | "nullable" | "many" | "nullable-many"

export type ReferenceSchema<
  S extends Schema.Top,
  C extends ReferenceCardinality,
> = C extends "one"
  ? S
  : C extends "nullable"
    ? Schema.NullOr<S>
    : C extends "many"
      ? Schema.$Array<S>
      : Schema.NullOr<Schema.$Array<S>>

declare module "effect/Schema" {
  namespace Annotations {
    interface Annotations {
      readonly appdb?: FieldMeta | undefined
    }
  }
}

type ReferenceFieldMeta<M extends AnyModel> =
  Omit<FieldMeta<M>, "foreign" | "relate"> &
    (
      | { readonly foreign: ForeignFieldMeta<M> }
      | { readonly relate: RelateFieldMeta<M> }
    )

/** Preserve target codec services and the source schema's native interface. */
type FieldWithReferenceServices<S extends Schema.Top, M extends AnyModel> =
  Omit<S, "DecodingServices" | "EncodingServices"> & {
    readonly DecodingServices: S["DecodingServices"] | M["schema"]["DecodingServices"]
    readonly EncodingServices: S["EncodingServices"] | M["schema"]["EncodingServices"]
  }

/**
 * Adds the same metadata to the decoded and encoded native schema views.
 * Storage planning therefore sees the metadata after `SchemaAST.toEncoded`.
 *
 * The reference overload also carries the target model's codec services. This
 * keeps a distinct-schema foreign field honest at the parent Effect boundary;
 * runtime target conversion remains the repository's responsibility.
 */
export function field<S extends Schema.Top, M extends AnyModel>(
  schema: S,
  metadata: ReferenceFieldMeta<M>,
): FieldWithReferenceServices<S, M>
export function field<S extends Schema.Top>(schema: S, metadata: FieldMeta): S
export function field<S extends Schema.Top>(schema: S, metadata: FieldMeta): Schema.Top {
  const annotatedMetadata: InternalFieldMeta = { ...metadata, [FIELD_SCHEMA]: schema }
  const annotated = schema.pipe(Schema.annotate({ appdb: annotatedMetadata }))
  return annotated.pipe(Schema.annotateEncoded({ appdb: annotatedMetadata })) as S
}

/** Return the exact schema instance wrapped by `field`, when available. */
export const fieldSchemaOf = (metadata: FieldMeta): Schema.Top | undefined =>
  (metadata as InternalFieldMeta)[FIELD_SCHEMA]

const referenceSchema = <M extends AnyModel, C extends ReferenceCardinality>(
  target: () => M,
  cardinality: C,
): ReferenceSchema<M["schema"], C> => {
  const leaf = Schema.suspend((): M["schema"] => target().schema)
  const wrapped: Schema.Top = cardinality === "one"
    ? leaf
    : cardinality === "nullable"
      ? Schema.NullOr(leaf)
      : cardinality === "many"
        ? Schema.Array(leaf)
        : Schema.NullOr(Schema.Array(leaf))
  return wrapped as ReferenceSchema<M["schema"], C>
}

/**
 * Builds a foreign field from the target model's exact native schema. The
 * target thunk remains lazy; compiling the owner never evaluates it.
 */
export function foreign<M extends AnyModel>(target: () => M): M["schema"]
export function foreign<M extends AnyModel, C extends ReferenceCardinality>(
  target: () => M,
  options: { readonly cardinality: C },
): ReferenceSchema<M["schema"], C>
export function foreign<M extends AnyModel, C extends ReferenceCardinality>(
  target: () => M,
  options: { readonly cardinality?: C } = {},
): Schema.Top {
  const cardinality = options.cardinality ?? ("one" as C)
  return field(referenceSchema(target, cardinality), { foreign: { target } }) as unknown as Schema.Top
}

export interface RelationOptions<C extends ReferenceCardinality = ReferenceCardinality> {
  readonly direction: RelationDirection
  readonly relation: string
  readonly cardinality?: C
}

/** Builds a relation field from the target model's exact native schema. */
export function relation<M extends AnyModel>(
  target: () => M,
  options: RelationOptions<"one">,
): M["schema"]
export function relation<M extends AnyModel, C extends ReferenceCardinality>(
  target: () => M,
  options: RelationOptions<C>,
): ReferenceSchema<M["schema"], C>
export function relation<M extends AnyModel, C extends ReferenceCardinality>(
  target: () => M,
  options: RelationOptions<C>,
): Schema.Top {
  const cardinality = options.cardinality ?? ("one" as C)
  return field(referenceSchema(target, cardinality), {
    relate: {
      target,
      direction: options.direction,
      relation: options.relation,
    },
  }) as unknown as Schema.Top
}

export interface StorageFieldPlan {
  readonly path: FieldPath
  readonly ast: SchemaAST.AST
  readonly encodedAst: SchemaAST.AST
  readonly metadata: FieldMeta
}

export interface CompiledSchemaPlan {
  readonly fields: readonly StorageFieldPlan[]
  readonly fieldNames: readonly string[]
  readonly idField?: string
  readonly uniqueFields: readonly string[]
  readonly paginationField?: string
}

const metadataAt = (ast: SchemaAST.AST): FieldMeta | undefined => {
  const value = SchemaAST.resolve(ast)?.appdb
  if (value === undefined || typeof value !== "object" || value === null) {
    return undefined
  }
  return value as FieldMeta
}

const isNullish = (ast: SchemaAST.AST): boolean => ast._tag === "Null" || ast._tag === "Undefined"

const stripNullish = (ast: SchemaAST.AST): SchemaAST.AST => {
  if (!SchemaAST.isUnion(ast)) {
    return ast
  }
  const members = ast.types.filter((member) => !isNullish(member))
  return members.length === 1 ? stripNullish(members[0]!) : ast
}

const arrayElement = (ast: SchemaAST.AST): SchemaAST.AST | undefined => {
  if (!SchemaAST.isArrays(ast) || ast.elements.length !== 0 || ast.rest.length !== 1) {
    return undefined
  }
  return ast.rest[0]
}

const validRelateShape = (ast: SchemaAST.AST): boolean => {
  const unwrapped = stripNullish(ast)
  const element = arrayElement(unwrapped)
  if (element === undefined) {
    return !SchemaAST.isArrays(unwrapped)
  }
  return arrayElement(stripNullish(element)) === undefined
}

const invalid = (message: string, operation = "compile model schema"): never => {
  throw new DbError({ kind: DbErrorKind.InvalidModel, message, operation })
}

const validateField = (fieldPlan: StorageFieldPlan): void => {
  const { metadata, path, encodedAst } = fieldPlan
  if (metadata.sensitive && (metadata.unique || metadata.pagination || metadata.relate)) {
    invalid(`sensitive field ${path.join(".")} cannot be unique, paginated, or related`)
  }
  if (metadata.foreign && metadata.relate) {
    invalid(`field ${path.join(".")} cannot be both foreign and relate`)
  }
  if (metadata.foreign && metadata.pagination) {
    invalid(`foreign field ${path.join(".")} cannot be paginated`)
  }
  if (metadata.relate && (metadata.pagination || !validRelateShape(encodedAst))) {
    invalid(`relate field ${path.join(".")} must be scalar, nullable, array, or nullable array`)
  }
}

const collectFields = (schema: Schema.Top): StorageFieldPlan[] => {
  const root = schema.ast
  const encodedRoot = SchemaAST.toEncoded(root)
  const fields: StorageFieldPlan[] = []
  const active = new Set<SchemaAST.AST>()

  const walk = (ast: SchemaAST.AST, encodedAst: SchemaAST.AST, path: FieldPath): void => {
    const metadata = metadataAt(encodedAst)
    if (metadata !== undefined) {
      fields.push({ path, ast, encodedAst, metadata })
      // A foreign or relation field is a storage boundary. Its target schema
      // owns the target's field metadata; walking through it would incorrectly
      // reclassify target id/unique/sensitive fields as nested parent fields.
      if (
        metadata.foreign !== undefined ||
        metadata.relate !== undefined ||
        metadata.sensitive?.scope === "value"
      ) {
        return
      }
    }
    if (active.has(ast)) {
      return
    }
    active.add(ast)
    try {
      if (SchemaAST.isObjects(ast) && SchemaAST.isObjects(encodedAst)) {
        for (const property of ast.propertySignatures) {
          const encodedProperty = encodedAst.propertySignatures.find(
            (candidate) => candidate.name === property.name,
          )
          if (encodedProperty !== undefined) {
            walk(property.type, encodedProperty.type, [...path, property.name])
          }
        }
      } else if (SchemaAST.isArrays(ast) && SchemaAST.isArrays(encodedAst)) {
        const encodedElements = encodedAst.elements
        for (const [index, element] of ast.elements.entries()) {
          const encodedElement = encodedElements[index]
          if (encodedElement !== undefined) {
            walk(element, encodedElement, [...path, index])
          }
        }
        for (const [index, element] of ast.rest.entries()) {
          const encodedElement = encodedAst.rest[index]
          if (encodedElement !== undefined) {
            walk(element, encodedElement, [...path, `*${index}`])
          }
        }
      } else if (SchemaAST.isUnion(ast) && SchemaAST.isUnion(encodedAst)) {
        for (const [index, member] of ast.types.entries()) {
          const encodedMember = encodedAst.types[index]
          if (encodedMember !== undefined) {
            walk(member, encodedMember, [...path, `|${index}`])
          }
        }
      } else if (SchemaAST.isSuspend(ast) && SchemaAST.isSuspend(encodedAst)) {
        walk(ast.thunk(), encodedAst.thunk(), path)
      }
    } finally {
      active.delete(ast)
    }
  }

  walk(root, encodedRoot, [])
  return fields
}

/** Compile all native schema metadata once; this function performs no I/O. */
export const compileSchemaPlan = (schema: Schema.Top): CompiledSchemaPlan => {
  const fields = collectFields(schema)
  for (const fieldPlan of fields) {
    validateField(fieldPlan)
  }

  const topLevel = fields.filter(
    (fieldPlan) => fieldPlan.path.length === 1 && typeof fieldPlan.path[0] === "string",
  )
  const idFields = topLevel.filter((fieldPlan) => fieldPlan.metadata.id)
  const paginationFields = topLevel.filter((fieldPlan) => fieldPlan.metadata.pagination)
  if (idFields.length > 1) {
    invalid("a model may declare at most one id field")
  }
  if (paginationFields.length > 1) {
    invalid("a model may declare at most one pagination field")
  }

  // Projection consumers need every declared root property, including fields
  // without appdb metadata (a View commonly selects ordinary fields). Metadata
  // plans remain the source for id/unique/pagination roles below.
  const encodedRoot = SchemaAST.toEncoded(schema.ast)
  const fieldNames = SchemaAST.isObjects(encodedRoot)
    ? encodedRoot.propertySignatures.map((property) => String(property.name))
    : topLevel.map((fieldPlan) => String(fieldPlan.path[0]))
  const uniqueFields = topLevel
    .filter((fieldPlan) => fieldPlan.metadata.unique)
    .map((fieldPlan) => String(fieldPlan.path[0]))

  const result: {
    fields: readonly StorageFieldPlan[]
    fieldNames: readonly string[]
    idField?: string
    uniqueFields: readonly string[]
    paginationField?: string
  } = { fields, fieldNames, uniqueFields }
  const idField = idFields[0]?.path[0]
  if (typeof idField === "string") {
    result.idField = idField
  }
  const paginationField = paginationFields[0]?.path[0]
  if (typeof paginationField === "string") {
    result.paginationField = paginationField
  }
  return result
}
