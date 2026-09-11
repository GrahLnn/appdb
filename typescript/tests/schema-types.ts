import { Context, Effect, Layer, Schema, SchemaGetter } from "effect"

import { Database } from "../src/connection.js"
import { Model } from "../src/model.js"
import { makeStore, type RepositoryError } from "../src/repository.js"
import { RootIdSchema } from "../src/id.js"
import { field } from "../src/schema.js"

interface CodecState {
  readonly prefix: string
}

class CodecEnv extends Context.Service<CodecEnv, CodecState>()("appdb/tests/SchemaTypesCodecEnv") {}

const childCodec = Schema.String.pipe(
  Schema.decodeTo(Schema.Number, {
    decode: SchemaGetter.transformEffect<number, string, CodecEnv>((encoded) =>
      CodecEnv.use((env) => Effect.succeed(Number(encoded.slice(env.prefix.length)))),
    ),
    encode: SchemaGetter.transformEffect<string, number, CodecEnv>((decoded) =>
      CodecEnv.use((env) => Effect.succeed(`${env.prefix}${decoded}`)),
    ),
  }),
)

const ChildSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  value: childCodec,
})
const Child = Model.define("schema_types_child", ChildSchema)

// The reference wrapper must retain optionalKey's native optionality marker.
const ParentSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  child: field(Schema.Unknown, { foreign: { target: () => Child } }),
  optionalChild: field(Schema.optionalKey(Child.schema), { foreign: { target: () => Child } }),
})
const Parent = Model.define("schema_types_parent", ParentSchema)
const omittedOptionalChild: typeof ParentSchema["Type"] = {
  id: "parent-1",
  child: {},
}
void omittedOptionalChild

// An Unknown fallback still delegates to the target model's codec, so its
// services must remain in the parent store effect environment.
const save = makeStore(Parent).save(omittedOptionalChild)
const saveWithTargetServices: Effect.Effect<
  typeof ParentSchema["Type"],
  RepositoryError,
  Database | CodecEnv
> = save
void saveWithTargetServices

const withCodecEnv = Layer.succeed(CodecEnv, { prefix: "n:" })
const saveAfterCodecEnv: Effect.Effect<
  typeof ParentSchema["Type"],
  RepositoryError,
  Database
> = Effect.provide(save, withCodecEnv)
void saveAfterCodecEnv

// @ts-expect-error Database and the target codec service are still required.
void Effect.runPromise(save)

