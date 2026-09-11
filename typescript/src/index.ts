/**
 * Cross-platform appdb API.
 *
 * This entry point deliberately excludes the Node native engine and OS
 * keyring provider. Import those from `@appdb/core/node` in Node processes.
 */
export * from "./connection.js"
export * from "./crypto.js"
export * from "./errors.js"
export * from "./id.js"
export * from "./model.js"
export * from "./pagination.js"
export * from "./query.js"
export * from "./tx.js"
export * from "./relation.js"
export * from "./graph.js"
export * from "./repository.js"
export * from "./schema-ddl.js"

export {
  compileSchemaPlan,
  field,
  fieldSchemaOf,
  foreign,
  relation,
  type CompiledSchemaPlan,
  type FieldMeta,
  type FieldPath,
  type ForeignFieldMeta,
  type ReferenceCardinality,
  type ReferenceSchema,
  type RelateFieldMeta,
  type RelationOptions,
  type RelationDirection,
  type SensitiveFieldMeta,
  type StorageFieldPlan,
} from "./schema.js"
