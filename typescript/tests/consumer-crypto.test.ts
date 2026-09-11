import { Effect, Context, Layer, Schema, SchemaGetter } from "effect"
import { TextEncoder } from "node:util"
import { describe, expect, it } from "vitest"

import { Database } from "../src/connection.js"
import { Crypto, CryptoError, makeStaticKeyLayer, type KeyContext } from "../src/crypto.js"
import { RootIdSchema } from "../src/id.js"
import { Model } from "../src/model.js"
import { makeNodeDatabaseLayer } from "../src/node.js"
import { makeStore } from "../src/repository.js"
import { field } from "../src/schema.js"

interface CodecState {
  readonly prefix: string
  encodeCalls: number
  decodeCalls: number
}

class CodecEnv extends Context.Service<CodecEnv, CodecState>()("appdb/tests/ConsumerCryptoCodecEnv") {}

const fixedKey = new Uint8Array(Array.from({ length: 32 }, (_, index) => index + 1))

const makeMemLayer = () =>
  makeNodeDatabaseLayer({
    endpoint: "mem://",
    namespace: "consumer-crypto",
    database: "consumer-crypto",
  })

const withServices = <A, E>(
  effect: Effect.Effect<A, E, Database | Crypto | CodecEnv>,
  state: CodecState,
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.provide(
        Effect.provide(
          Effect.provide(effect, makeMemLayer()),
          makeStaticKeyLayer(fixedKey),
        ),
        Layer.succeed(CodecEnv, state),
      ),
    ),
  )

const prefixedNumber = Schema.String.pipe(
  Schema.decodeTo(Schema.Number, {
    decode: SchemaGetter.transformEffect<number, string, CodecEnv>((encoded) =>
      CodecEnv.use((env) => {
        env.decodeCalls += 1
        return Effect.succeed(Number(encoded.slice(env.prefix.length)))
      }),
    ),
    encode: SchemaGetter.transformEffect<string, number, CodecEnv>((decoded) =>
      CodecEnv.use((env) => {
        env.encodeCalls += 1
        return Effect.succeed(`${env.prefix}${decoded}`)
      }),
    ),
  }),
)

const SensitiveSchema = Schema.Struct({
  id: field(RootIdSchema, { id: true }),
  leaf: field(Schema.String, { sensitive: { scope: "leaf" } }),
  value: field(
    Schema.NullOr(Schema.Array(Schema.NullOr(Schema.String))),
    { sensitive: { scope: "value" } },
  ),
  coded: field(prefixedNumber, { sensitive: { scope: "leaf" } }),
})

const Sensitive = Model.define("consumer_crypto_secret", SensitiveSchema)
const store = makeStore(Sensitive)

const rawRows = (slots: readonly unknown[]): readonly Record<string, unknown>[] => {
  const slot = slots[0]
  return Array.isArray(slot) ? slot as readonly Record<string, unknown>[] : []
}

const asBytes = (value: unknown): Uint8Array => {
  if (value instanceof Uint8Array) return value
  if (value instanceof ArrayBuffer) return new Uint8Array(value)
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
  }
  if (Array.isArray(value) && value.every((item) => typeof item === "number")) {
    return Uint8Array.from(value)
  }
  throw new TypeError("expected a byte value from the database")
}

const isByteContainer = (value: unknown): boolean =>
  value instanceof ArrayBuffer || ArrayBuffer.isView(value)

describe("sensitive fields at the real store consumer", () => {
  it("saves/gets leaf and nullable-array value fields while raw storage stays encrypted", async () => {
    const state: CodecState = { prefix: "n:", encodeCalls: 0, decodeCalls: 0 }
    const input = {
      id: "one",
      leaf: "leaf-secret",
      value: ["first", null],
      coded: 7,
    }
    const nullableInput = {
      id: "two",
      leaf: "another-secret",
      value: null,
      coded: 8,
    }

    const result = await withServices(
      Effect.gen(function* () {
        const database = yield* Database
        const beforeSave = { encode: state.encodeCalls, decode: state.decodeCalls }
        const saved = yield* store.save(input)
        const saveCalls = {
          encode: state.encodeCalls - beforeSave.encode,
          decode: state.decodeCalls - beforeSave.decode,
        }

        const nullableSaved = yield* store.save(nullableInput)
        const rows = rawRows(yield* database.query(
          "SELECT *, record::id(id) AS id FROM consumer_crypto_secret ORDER BY id ASC;",
        ))
        const firstRaw = rows.find((row) => row.id === "one")
        const secondRaw = rows.find((row) => row.id === "two")
        if (firstRaw === undefined || secondRaw === undefined) {
          throw new Error("sensitive rows were not returned by the raw query")
        }

        const beforeGet = { encode: state.encodeCalls, decode: state.decodeCalls }
        const fetched = yield* store.get("one")
        const getCalls = {
          encode: state.encodeCalls - beforeGet.encode,
          decode: state.decodeCalls - beforeGet.decode,
        }
        const nullableFetched = yield* store.get("two")
        const crypto = yield* Crypto
        const context: KeyContext = { model: Sensitive.table, field: "value" }
        const decodedRawValue = yield* crypto.decryptValue<typeof input.value>(
          asBytes(firstRaw.value),
          context,
        )
        return {
          saved,
          nullableSaved,
          fetched,
          nullableFetched,
          firstRaw,
          secondRaw,
          decodedRawValue,
          saveCalls,
          getCalls,
        }
      }),
      state,
    )

    expect(result.saved).toEqual(input)
    expect(result.nullableSaved).toEqual(nullableInput)
    expect(result.fetched).toEqual(input)
    expect(result.nullableFetched).toEqual(nullableInput)
    expect(isByteContainer(result.firstRaw.leaf)).toBe(true)
    expect(isByteContainer(result.firstRaw.value)).toBe(true)
    expect(isByteContainer(result.firstRaw.coded)).toBe(true)
    expect(asBytes(result.firstRaw.leaf)).toHaveLength(
      12 + new TextEncoder().encode(JSON.stringify(input.leaf)).byteLength + 16,
    )
    expect(asBytes(result.firstRaw.value)).toHaveLength(
      12 + new TextEncoder().encode(JSON.stringify(input.value)).byteLength + 16,
    )
    expect(asBytes(result.firstRaw.coded)).toHaveLength(
      12 + new TextEncoder().encode(JSON.stringify("n:7")).byteLength + 16,
    )
    expect(asBytes(result.firstRaw.leaf)).not.toEqual(new TextEncoder().encode(input.leaf))
    expect(result.secondRaw.value).toBeNull()
    expect(result.decodedRawValue).toEqual(input.value)
    expect(result.saveCalls).toEqual({ encode: 1, decode: 1 })
    expect(result.getCalls).toEqual({ encode: 0, decode: 1 })
  })

  it("surfaces a tampered stored ciphertext as a typed crypto error", async () => {
    const state: CodecState = { prefix: "n:", encodeCalls: 0, decodeCalls: 0 }
    const result = await withServices(
      Effect.gen(function* () {
        const database = yield* Database
        yield* store.save({ id: "tampered", leaf: "secret", value: ["x"], coded: 1 })
        const rows = rawRows(yield* database.query(
          "SELECT *, record::id(id) AS id FROM consumer_crypto_secret:tampered;",
        ))
        const row = rows[0]
        if (row === undefined) throw new Error("tampered row was not returned")
        const ciphertext = asBytes(row.leaf)
        ciphertext[ciphertext.length - 1] = ciphertext[ciphertext.length - 1]! ^ 1
        yield* database.query(
          "UPDATE consumer_crypto_secret:tampered MERGE { leaf: $leaf };",
          { leaf: ciphertext },
        )
        return yield* Effect.flip(store.get("tampered"))
      }),
      state,
    )

    expect(result).toBeInstanceOf(CryptoError)
    if (result instanceof CryptoError) {
      expect(result.reason).toBe("decrypt")
    }
  })
})

describe("public Node crypto entry", () => {
  it("imports the NodeKeyring export without opening a real credential store", async () => {
    const nodeEntry = await import("../src/node-entry.js")
    expect(typeof nodeEntry.makeNodeKeyringProvider).toBe("function")
    expect(typeof nodeEntry.makeNodeKeyringLayer).toBe("function")

    let reads = 0
    const provider = nodeEntry.makeNodeKeyringProvider({
      entryFactory: async () => ({
        getPassword: async () => {
          reads += 1
          return "0123456789abcdef".repeat(4)
        },
        setPassword: async () => undefined,
      }),
    })
    const key = await provider.loadKey({ model: "Public", field: "token" })
    expect(key).toHaveLength(32)
    expect(reads).toBe(1)
  })
})
