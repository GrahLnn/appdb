import { Context, Data, Effect, Layer } from "effect"

export const CRYPTO_KEY_BYTES = 32
export const CRYPTO_NONCE_BYTES = 12
export const CRYPTO_TAG_BYTES = 16

export const DEFAULT_CRYPTO_SERVICE = "appdb"
export const DEFAULT_CRYPTO_ACCOUNT = "master-sensitive"

/** The stable model/field identity used to select a symmetric key. */
export interface KeyContext {
  readonly model: string
  readonly field: string
  readonly service?: string
  readonly account?: string
}

/** A context with the Rust implementation's service/account defaults applied. */
export interface ResolvedKeyContext {
  readonly model: string
  readonly field: string
  readonly service: string
  readonly account: string
}

export const resolveKeyContext = (context: KeyContext): ResolvedKeyContext => ({
  model: context.model,
  field: context.field,
  service: context.service ?? DEFAULT_CRYPTO_SERVICE,
  account: context.account ?? DEFAULT_CRYPTO_ACCOUNT,
})

export type CryptoErrorReason =
  | "invalid-key"
  | "key-provider"
  | "ciphertext-too-short"
  | "encrypt"
  | "decrypt"
  | "utf8"
  | "json-encode"
  | "json-decode"

/** Expected failures from key resolution, encoding, or authenticated encryption. */
export class CryptoError extends Data.TaggedError("CryptoError")<{
  readonly reason: CryptoErrorReason
  readonly message: string
  readonly context?: KeyContext
  readonly cause?: unknown
}> {}

export const isCryptoError = (value: unknown): value is CryptoError =>
  typeof value === "object" && value !== null && "_tag" in value && value._tag === "CryptoError"

export const makeCryptoError = (
  reason: CryptoErrorReason,
  message: string,
  context?: KeyContext,
  cause?: unknown,
): CryptoError => {
  const fields: {
    readonly reason: CryptoErrorReason
    readonly message: string
    readonly context?: KeyContext
    readonly cause?: unknown
  } = context === undefined
    ? cause === undefined
      ? { reason, message }
      : { reason, message, cause }
    : cause === undefined
      ? { reason, message, context }
      : { reason, message, context, cause }
  return new CryptoError(fields)
}

/** A key provider is intentionally instance-owned; it must not be process-global. */
export interface KeyProvider {
  readonly loadKey: (
    context: KeyContext,
    signal?: AbortSignal,
  ) => PromiseLike<Uint8Array> | Uint8Array
}

export interface WebCryptoLike {
  readonly subtle: typeof globalThis.crypto.subtle
  readonly getRandomValues: typeof globalThis.crypto.getRandomValues
}

export interface CryptoApi {
  readonly encryptBytes: (
    value: Uint8Array,
    context: KeyContext,
  ) => Effect.Effect<Uint8Array, CryptoError>
  readonly decryptBytes: (
    value: Uint8Array,
    context: KeyContext,
  ) => Effect.Effect<Uint8Array, CryptoError>
  readonly encryptText: (
    value: string,
    context: KeyContext,
  ) => Effect.Effect<Uint8Array, CryptoError>
  readonly decryptText: (
    value: Uint8Array,
    context: KeyContext,
  ) => Effect.Effect<string, CryptoError>
  readonly encryptValue: <A>(
    value: A,
    context: KeyContext,
  ) => Effect.Effect<Uint8Array, CryptoError>
  readonly decryptValue: <A = unknown>(
    value: Uint8Array,
    context: KeyContext,
  ) => Effect.Effect<A, CryptoError>
}

/** Effect service for the shared AES-256-GCM and value codec boundary. */
export class Crypto extends Context.Service<Crypto, CryptoApi>()("appdb/crypto/Crypto") {}

/** Alias used by callers that prefer the explicit service name. */
export { Crypto as CryptoService }

const encoder = new globalThis.TextEncoder()
const decoder = new globalThis.TextDecoder("utf-8", { fatal: true })

const copyBytes = (value: Uint8Array): Uint8Array => new Uint8Array(value)

export const isByteContainer = (value: unknown): boolean =>
  value instanceof Uint8Array ||
  value instanceof ArrayBuffer ||
  ArrayBuffer.isView(value) ||
  (Array.isArray(value) && value.every((item) =>
    typeof item === "number" && Number.isInteger(item) && item >= 0 && item <= 255,
  ))

/**
 * Normalize bytes returned by a storage driver without widening a view to its
 * entire backing buffer. Uint8Array and views preserve their existing window;
 * ArrayBuffer gets a zero-offset view; numeric arrays remain a compatibility
 * path for JSON transports.
 */
export const normalizeBytes = (value: unknown): Uint8Array => {
  if (value instanceof Uint8Array) return value
  if (value instanceof ArrayBuffer) return new Uint8Array(value)
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer as ArrayBuffer, value.byteOffset, value.byteLength)
  }
  if (isByteContainer(value) && Array.isArray(value)) {
    return Uint8Array.from(value)
  }
  throw new TypeError("expected a byte container")
}

const keyCacheKey = (context: ResolvedKeyContext): string =>
  JSON.stringify([context.model, context.field, context.service, context.account])

const validateKey = (key: Uint8Array, context: KeyContext): Uint8Array => {
  if (key.byteLength !== CRYPTO_KEY_BYTES) {
    throw makeCryptoError(
      "invalid-key",
      `crypto key must be exactly ${CRYPTO_KEY_BYTES} bytes`,
      context,
    )
  }
  return copyBytes(key)
}

const encodeOperationError = (
  reason: Extract<CryptoErrorReason, "encrypt" | "decrypt">,
  context: KeyContext,
  cause: unknown,
): CryptoError =>
  isCryptoError(cause)
    ? cause
    : makeCryptoError(reason, `${reason} operation failed`, context, cause)

const getKeyEffect = (
  provider: KeyProvider,
  cache: Map<string, Uint8Array>,
  inFlight: Map<string, Promise<Uint8Array>>,
  context: KeyContext,
): Effect.Effect<Uint8Array, CryptoError> => {
  const resolved = resolveKeyContext(context)
  const cacheKey = keyCacheKey(resolved)
  return Effect.tryPromise({
    try: (signal) => {
      const cached = cache.get(cacheKey)
      if (cached !== undefined) {
        return Promise.resolve(copyBytes(cached))
      }

      let loading = inFlight.get(cacheKey)
      if (loading === undefined) {
        loading = Promise.resolve()
          .then(() => provider.loadKey(resolved, signal))
          .then((key) => validateKey(key, context))
        inFlight.set(cacheKey, loading)
        void loading.then(
          (key) => {
            if (inFlight.get(cacheKey) === loading) {
              inFlight.delete(cacheKey)
              cache.set(cacheKey, copyBytes(key))
            }
          },
          () => {
            if (inFlight.get(cacheKey) === loading) {
              inFlight.delete(cacheKey)
            }
          },
        )
      }

      // The shared promise must remain alive for other callers. Effect still
      // observes interruption; the provider receives the signal on first load.
      return loading
    },
    catch: (cause) =>
      isCryptoError(cause)
        ? cause
        : makeCryptoError("key-provider", "key provider failed", context, cause),
  })
}

type AesCryptoKey = Awaited<ReturnType<typeof globalThis.crypto.subtle.importKey>>

const importAesKey = async (crypto: WebCryptoLike, key: Uint8Array): Promise<AesCryptoKey> =>
  crypto.subtle.importKey("raw", key, { name: "AES-GCM" }, false, ["encrypt", "decrypt"])

const concatBytes = (first: Uint8Array, second: Uint8Array): Uint8Array => {
  const result = new Uint8Array(first.byteLength + second.byteLength)
  result.set(first, 0)
  result.set(second, first.byteLength)
  return result
}

const encryptWithKey = async (
  value: Uint8Array,
  key: Uint8Array,
  crypto: WebCryptoLike,
): Promise<Uint8Array> => {
  const nonce = new Uint8Array(CRYPTO_NONCE_BYTES)
  crypto.getRandomValues(nonce)
  const cryptoKey = await importAesKey(crypto, key)
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv: nonce, tagLength: CRYPTO_TAG_BYTES * 8 },
    cryptoKey,
    value,
  )
  return concatBytes(nonce, new Uint8Array(ciphertext))
}

const decryptWithKey = async (
  value: Uint8Array,
  key: Uint8Array,
  crypto: WebCryptoLike,
): Promise<Uint8Array> => {
  if (value.byteLength < CRYPTO_NONCE_BYTES + CRYPTO_TAG_BYTES) {
    throw makeCryptoError(
      "ciphertext-too-short",
      `ciphertext must contain a ${CRYPTO_NONCE_BYTES}-byte nonce and ${CRYPTO_TAG_BYTES}-byte tag`,
    )
  }
  const nonce = value.subarray(0, CRYPTO_NONCE_BYTES)
  const ciphertext = value.subarray(CRYPTO_NONCE_BYTES)
  const cryptoKey = await importAesKey(crypto, key)
  const plaintext = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: nonce, tagLength: CRYPTO_TAG_BYTES * 8 },
    cryptoKey,
    ciphertext,
  )
  return new Uint8Array(plaintext)
}

const encodeJson = (value: unknown): string => {
  const serialized = JSON.stringify(value, (_key, current: unknown) => {
    if (typeof current === "bigint") {
      throw new TypeError("bigint values are not JSON-compatible")
    }
    if (typeof current === "undefined" || typeof current === "function" || typeof current === "symbol") {
      throw new TypeError("undefined, function, and symbol values are not JSON-compatible")
    }
    if (typeof current === "number") {
      if (!Number.isFinite(current)) {
        throw new TypeError("non-finite numbers are not JSON-compatible")
      }
      if (Number.isInteger(current) && !Number.isSafeInteger(current)) {
        throw new TypeError("integer values outside the safe range are not JSON-compatible")
      }
    }
    return current
  })
  if (serialized === undefined) {
    throw new TypeError("value is not JSON-compatible")
  }
  return serialized
}

const makeCryptoApi = (provider: KeyProvider, crypto: WebCryptoLike): CryptoApi => {
  // These maps intentionally live in this service instance. A layer build gets
  // its own provider/cache boundary, so separate apps cannot share key state.
  const cache = new Map<string, Uint8Array>()
  const inFlight = new Map<string, Promise<Uint8Array>>()

  const key = (context: KeyContext): Effect.Effect<Uint8Array, CryptoError> =>
    getKeyEffect(provider, cache, inFlight, context)

  const encryptBytes = (value: Uint8Array, context: KeyContext): Effect.Effect<Uint8Array, CryptoError> =>
    key(context).pipe(
      Effect.flatMap((resolvedKey) =>
        Effect.tryPromise({
          try: () => encryptWithKey(value, resolvedKey, crypto),
          catch: (cause) => encodeOperationError("encrypt", context, cause),
        }),
      ),
    )

  const decryptBytes = (value: Uint8Array, context: KeyContext): Effect.Effect<Uint8Array, CryptoError> =>
    key(context).pipe(
      Effect.flatMap((resolvedKey) =>
        Effect.tryPromise({
          try: () => decryptWithKey(value, resolvedKey, crypto),
          catch: (cause) => encodeOperationError("decrypt", context, cause),
        }),
      ),
    )

  const encryptText = (value: string, context: KeyContext): Effect.Effect<Uint8Array, CryptoError> =>
    encryptBytes(encoder.encode(value), context)

  const decryptText = (value: Uint8Array, context: KeyContext): Effect.Effect<string, CryptoError> =>
    decryptBytes(value, context).pipe(
      Effect.flatMap((plaintext) =>
        Effect.try({
          try: () => decoder.decode(plaintext),
          catch: (cause) => makeCryptoError("utf8", "decrypted data is not valid UTF-8", context, cause),
        }),
      ),
    )

  const encryptValue = <A>(value: A, context: KeyContext): Effect.Effect<Uint8Array, CryptoError> =>
    Effect.try({
      try: () => encodeJson(value),
      catch: (cause) => makeCryptoError("json-encode", "value is not compact-JSON compatible", context, cause),
    }).pipe(Effect.flatMap((json) => encryptText(json, context)))

  const decryptValue = <A = unknown>(value: Uint8Array, context: KeyContext): Effect.Effect<A, CryptoError> =>
    decryptText(value, context).pipe(
      Effect.flatMap((json) =>
        Effect.try({
          try: () => JSON.parse(json) as A,
          catch: (cause) => makeCryptoError("json-decode", "decrypted data is not valid JSON", context, cause),
        }),
      ),
    )

  return { encryptBytes, decryptBytes, encryptText, decryptText, encryptValue, decryptValue }
}

export interface CryptoLayerOptions {
  readonly webCrypto?: WebCryptoLike
}

/** Builds an isolated Crypto service layer around one injected key provider. */
export const makeCryptoLayer = (
  provider: KeyProvider,
  options: CryptoLayerOptions = {},
): Layer.Layer<Crypto, never, never> =>
  Layer.effect(
    Crypto,
    Effect.sync(() => Crypto.of(makeCryptoApi(provider, options.webCrypto ?? globalThis.crypto))),
  )

/** Builds a provider that returns an explicit 32-byte key for tests/configuration. */
export const makeStaticKeyProvider = (key: Uint8Array): KeyProvider => {
  const configured = copyBytes(key)
  return {
    loadKey: (context) => {
      if (configured.byteLength !== CRYPTO_KEY_BYTES) {
        return Promise.reject(
          makeCryptoError(
            "invalid-key",
            `crypto key must be exactly ${CRYPTO_KEY_BYTES} bytes`,
            context,
          ),
        )
      }
      return copyBytes(configured)
    },
  }
}

/** Explicit static-key layer; it never reads or writes an OS credential store. */
export const makeStaticKeyLayer = (
  key: Uint8Array,
  options: CryptoLayerOptions = {},
): Layer.Layer<Crypto, never, never> => makeCryptoLayer(makeStaticKeyProvider(key), options)

export const CryptoLive = makeCryptoLayer
export const StaticKeyLayer = makeStaticKeyLayer
