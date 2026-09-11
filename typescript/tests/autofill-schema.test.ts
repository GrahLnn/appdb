import { Effect, Schema, SchemaGetter } from "effect"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeStore } from "../src/repository.js"
import { RootIdSchema } from "../src/id.js"
import { field } from "../src/schema.js"

interface Counters {
  fillCalls: number
  autoEncodeCalls: number
  autoDecodeCalls: number
  rootEncodeCalls: number
  rootDecodeCalls: number
  childEncodeCalls: number
  childDecodeCalls: number
}

const withMemDatabase = <A, E>(effect: Effect.Effect<A, E, Database>) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        effect,
        makeNodeDatabaseLayer({
          endpoint: "mem://",
          namespace: "autofill-schema",
          database: "autofill-schema",
        }),
      ),
    ),
  )

/**
 * An encode-side default expressed with the native Effect Schema codec API.
 * The decoded input intentionally remains `string | undefined`, so `undefined`
 * is the pending state. Encoding resolves it immediately before Store writes.
 */
const makeAutoFillSchema = (counters: Counters) => {
  const pendingOrResolved = Schema.Union([Schema.String, Schema.Undefined])
  return pendingOrResolved.pipe(
    Schema.decodeTo(pendingOrResolved, {
      decode: SchemaGetter.transformEffect<string | undefined, string | undefined>((value) => {
        counters.autoDecodeCalls += 1
        return Effect.succeed(value)
      }),
      encode: SchemaGetter.withDefault(
        Effect.sync(() => {
          counters.fillCalls += 1
          return `now-${counters.fillCalls}`
        }),
      ).map((value) => {
        counters.autoEncodeCalls += 1
        return value
      }),
    }),
  )
}

const makeCountedStringSchema = (
  onEncode: () => void,
  onDecode: () => void,
) => Schema.String.pipe(
  Schema.decodeTo(Schema.String, {
    decode: SchemaGetter.transformEffect<string, string>((value) => {
      onDecode()
      return Effect.succeed(value)
    }),
    encode: SchemaGetter.transformEffect<string, string>((value) => {
      onEncode()
      return Effect.succeed(value)
    }),
  }),
)

describe("native Schema encode-side autofill", () => {
  it("resolves pending values once before a foreign write and never fills on reads", async () => {
    const counters: Counters = {
      fillCalls: 0,
      autoEncodeCalls: 0,
      autoDecodeCalls: 0,
      rootEncodeCalls: 0,
      rootDecodeCalls: 0,
      childEncodeCalls: 0,
      childDecodeCalls: 0,
    }
    const autoFill = makeAutoFillSchema(counters)
    const ChildSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      filledAt: autoFill,
      label: makeCountedStringSchema(
        () => { counters.childEncodeCalls += 1 },
        () => { counters.childDecodeCalls += 1 },
      ),
    })
    const Child = Model.define("autofill_schema_child", ChildSchema)
    const ParentSchema = Schema.Struct({
      id: field(RootIdSchema, { id: true }),
      child: field(ChildSchema, { foreign: { target: () => Child } }),
      marker: makeCountedStringSchema(
        () => { counters.rootEncodeCalls += 1 },
        () => { counters.rootDecodeCalls += 1 },
      ),
    })
    const Parent = Model.define("autofill_schema_parent", ParentSchema)
    const store = makeStore(Parent)

    const result = await withMemDatabase(
      Effect.gen(function* () {
        const pending = yield* store.save({
          id: "pending-parent",
          child: { id: "pending-child", filledAt: undefined, label: "pending" },
          marker: "pending-root",
        })
        expect(pending.child.filledAt).toBe("now-1")
        expect(counters).toEqual({
          fillCalls: 1,
          autoEncodeCalls: 1,
          autoDecodeCalls: 1,
          rootEncodeCalls: 1,
          rootDecodeCalls: 1,
          childEncodeCalls: 1,
          childDecodeCalls: 1,
        })

        const beforePendingRead = { ...counters }
        const readPending = yield* store.get("pending-parent")
        expect(readPending.child.filledAt).toBe("now-1")
        expect(counters.fillCalls).toBe(beforePendingRead.fillCalls)
        expect(counters.autoEncodeCalls).toBe(beforePendingRead.autoEncodeCalls)
        expect(counters.rootEncodeCalls).toBe(beforePendingRead.rootEncodeCalls)
        expect(counters.childEncodeCalls).toBe(beforePendingRead.childEncodeCalls)
        expect(counters.autoDecodeCalls).toBe(beforePendingRead.autoDecodeCalls + 1)
        expect(counters.rootDecodeCalls).toBe(beforePendingRead.rootDecodeCalls + 1)
        expect(counters.childDecodeCalls).toBe(beforePendingRead.childDecodeCalls + 1)

        const resolved = yield* store.save({
          id: "resolved-parent",
          child: { id: "resolved-child", filledAt: "explicit", label: "resolved" },
          marker: "resolved-root",
        })
        expect(resolved.child.filledAt).toBe("explicit")
        expect(counters.fillCalls).toBe(1)
        expect(counters.autoEncodeCalls).toBe(2)
        expect(counters.rootEncodeCalls).toBe(2)
        expect(counters.childEncodeCalls).toBe(2)

        const beforeResolvedRead = { ...counters }
        const readResolved = yield* store.get("resolved-parent")
        expect(readResolved.child.filledAt).toBe("explicit")
        expect(counters.fillCalls).toBe(beforeResolvedRead.fillCalls)
        expect(counters.autoEncodeCalls).toBe(beforeResolvedRead.autoEncodeCalls)
        expect(counters.rootEncodeCalls).toBe(beforeResolvedRead.rootEncodeCalls)
        expect(counters.childEncodeCalls).toBe(beforeResolvedRead.childEncodeCalls)
        expect(counters.autoDecodeCalls).toBe(beforeResolvedRead.autoDecodeCalls + 1)
        expect(counters.rootDecodeCalls).toBe(beforeResolvedRead.rootDecodeCalls + 1)
        expect(counters.childDecodeCalls).toBe(beforeResolvedRead.childDecodeCalls + 1)

        return { pending, resolved, readPending, readResolved }
      }),
    )

    expect(result.pending.child.filledAt).toBe("now-1")
    expect(result.resolved.child.filledAt).toBe("explicit")
    expect(result.readPending.child.filledAt).toBe("now-1")
    expect(result.readResolved.child.filledAt).toBe("explicit")
  })
})
