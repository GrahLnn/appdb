import { Effect } from "effect"
import { describe, expect, it } from "vitest"
import {
  CRYPTO_KEY_BYTES,
  CRYPTO_NONCE_BYTES,
  CRYPTO_TAG_BYTES,
  Crypto,
  CryptoError,
  makeCryptoLayer,
  makeStaticKeyLayer,
  type KeyContext,
} from "../src/crypto.js"
import { makeNodeKeyringProvider, type AsyncKeyringEntry } from "../src/node-keyring.js"

const context: KeyContext = { model: "User", field: "token" }
const fixedKey = new Uint8Array(Array.from({ length: CRYPTO_KEY_BYTES }, (_, index) => index + 1))

const withCrypto = <A, E>(effect: Effect.Effect<A, E, Crypto>, layer = makeStaticKeyLayer(fixedKey)) =>
  Effect.runPromise(Effect.provide(effect, layer))

describe("Crypto service", () => {
  it("round-trips bytes and keeps the Rust wire layout", async () => {
    const plaintext = new Uint8Array([0, 1, 2, 255])
    const result = await withCrypto(
      Effect.gen(function* () {
        const crypto = yield* Crypto
        const ciphertext = yield* crypto.encryptBytes(plaintext, context)
        const decrypted = yield* crypto.decryptBytes(ciphertext, context)
        return { ciphertext, decrypted }
      }),
    )

    expect(result.ciphertext.byteLength).toBe(
      CRYPTO_NONCE_BYTES + plaintext.byteLength + CRYPTO_TAG_BYTES,
    )
    expect(result.decrypted).toEqual(plaintext)
    expect(result.ciphertext.subarray(0, CRYPTO_NONCE_BYTES)).not.toEqual(
      new Uint8Array(CRYPTO_NONCE_BYTES),
    )
  })

  it("uses fresh random nonces and supports text/value codecs", async () => {
    const result = await withCrypto(
      Effect.gen(function* () {
        const crypto = yield* Crypto
        const first = yield* crypto.encryptText("你好", context)
        const second = yield* crypto.encryptText("你好", context)
        const value = yield* crypto.encryptValue({ n: 3, text: "ok" }, context)
        const decoded = yield* crypto.decryptText(first, context)
        const decodedValue = yield* crypto.decryptValue<{ n: number; text: string }>(value, context)
        return { first, second, decoded, decodedValue }
      }),
    )

    expect(result.first).not.toEqual(result.second)
    expect(result.decoded).toBe("你好")
    expect(result.decodedValue).toEqual({ n: 3, text: "ok" })
  })

  it("returns tagged errors for tampering, short input, and invalid JSON values", async () => {
    const result = await withCrypto(
      Effect.gen(function* () {
        const crypto = yield* Crypto
        const ciphertext = yield* crypto.encryptText("secret", context)
        const tampered = new Uint8Array(ciphertext)
        tampered[tampered.length - 1] = tampered[tampered.length - 1]! ^ 1
        const tamperError = yield* Effect.flip(crypto.decryptText(tampered, context))
        const shortError = yield* Effect.flip(crypto.decryptBytes(new Uint8Array(3), context))
        const jsonError = yield* Effect.flip(crypto.encryptValue(1n, context))
        return { tamperError, shortError, jsonError }
      }),
    )

    expect(result.tamperError).toBeInstanceOf(CryptoError)
    expect(result.tamperError.reason).toBe("decrypt")
    expect(result.shortError).toBeInstanceOf(CryptoError)
    expect(result.shortError.reason).toBe("ciphertext-too-short")
    expect(result.jsonError).toBeInstanceOf(CryptoError)
    expect(result.jsonError.reason).toBe("json-encode")
  })

  it("single-flights key loading inside one layer instance", async () => {
    let calls = 0
    const provider = {
      loadKey: async () => {
        calls += 1
        await new Promise((resolve) => setTimeout(resolve, 10))
        return fixedKey
      },
    }
    const result = await withCrypto(
      Effect.gen(function* () {
        const crypto = yield* Crypto
        return yield* Effect.all(
          [
            crypto.encryptText("a", context),
            crypto.encryptText("b", context),
            crypto.encryptText("c", context),
          ],
          { concurrency: "unbounded" },
        )
      }),
      makeCryptoLayer(provider),
    )

    expect(result).toHaveLength(3)
    expect(calls).toBe(1)
  })

  it("does not share a static provider's key across service layers", async () => {
    const first = new Uint8Array(fixedKey)
    const second = new Uint8Array(fixedKey)
    second[0] = 99
    let firstCalls = 0
    let secondCalls = 0
    const firstProvider = {
      loadKey: async () => {
        firstCalls += 1
        return first
      },
    }
    const secondProvider = {
      loadKey: async () => {
        secondCalls += 1
        return second
      },
    }
    await withCrypto(Effect.gen(function* () {
      const crypto = yield* Crypto
      yield* crypto.encryptText("a", context)
    }), makeCryptoLayer(firstProvider))
    await withCrypto(Effect.gen(function* () {
      const crypto = yield* Crypto
      yield* crypto.encryptText("a", context)
    }), makeCryptoLayer(secondProvider))

    expect(firstCalls).toBe(1)
    expect(secondCalls).toBe(1)
  })
})

describe("Node keyring provider", () => {
  const makeEntry = (password: string | null | undefined, setPassword: AsyncKeyringEntry["setPassword"] = async () => {}) => {
    let reads = 0
    const entry: AsyncKeyringEntry = {
      getPassword: async () => {
        reads += 1
        return password
      },
      setPassword,
    }
    return { entry, get reads() { return reads } }
  }

  it("keeps Rust's lowercase 64-hex key and only generates on a missing entry", async () => {
    const generated = new Uint8Array(Array.from({ length: CRYPTO_KEY_BYTES }, (_, index) => 255 - index))
    let stored: string | undefined
    const fake = makeEntry(undefined, async (value) => { stored = value })
    const provider = makeNodeKeyringProvider({
      entryFactory: async () => fake.entry,
      randomKey: () => generated,
    })

    const loaded = await provider.loadKey({ ...context, service: "appdb", account: "master-sensitive" })
    expect(loaded).toEqual(generated)
    expect(stored).toBe("fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0")
  })

  it("propagates provider failures and rejects malformed values without replacing them", async () => {
    let writes = 0
    const providerFailure = makeNodeKeyringProvider({
      entryFactory: async () => ({
        getPassword: async () => { throw new Error("locked") },
        setPassword: async () => { writes += 1 },
      }),
    })
    await expect(providerFailure.loadKey({ ...context })).rejects.toThrow("locked")

    const malformed = makeNodeKeyringProvider({
      entryFactory: async () => ({
        getPassword: async () => "",
        setPassword: async () => { writes += 1 },
      }),
    })
    await expect(malformed.loadKey({ ...context })).rejects.toMatchObject({
      _tag: "CryptoError",
      reason: "invalid-key",
    })
    expect(writes).toBe(0)
  })

  it("accepts a stored 64-hex value without generating or rewriting it", async () => {
    const stored = "0123456789abcdef".repeat(4)
    let writes = 0
    const provider = makeNodeKeyringProvider({
      entryFactory: async () => ({
        getPassword: async () => stored,
        setPassword: async () => { writes += 1 },
      }),
      randomKey: () => { throw new Error("must not generate") },
    })

    await expect(provider.loadKey({ ...context })).resolves.toEqual(
      new Uint8Array([1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239, 1, 35, 69, 103, 137, 171, 205, 239]),
    )
    expect(writes).toBe(0)
  })
})
