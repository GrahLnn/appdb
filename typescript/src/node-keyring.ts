import { webcrypto as nodeWebCrypto } from "node:crypto"
import { Layer } from "effect"
import {
  type CryptoLayerOptions,
  Crypto,
  type KeyContext,
  type KeyProvider,
  type ResolvedKeyContext,
  makeCryptoLayer,
  makeCryptoError,
  resolveKeyContext,
  CRYPTO_KEY_BYTES,
} from "./crypto.js"

/** The async subset of @napi-rs/keyring used by the production provider. */
export interface AsyncKeyringEntry {
  readonly getPassword: (signal?: AbortSignal | null) => Promise<string | null | undefined>
  readonly setPassword: (password: string, signal?: AbortSignal | null) => Promise<void>
}

export type KeyringEntryFactory = (
  service: string,
  account: string,
) => AsyncKeyringEntry | PromiseLike<AsyncKeyringEntry>

export interface NodeKeyringProviderOptions {
  /** Injectable seam for tests; the default dynamically loads the native addon. */
  readonly entryFactory?: KeyringEntryFactory
  /** Injectable WebCrypto random source for deterministic provider tests. */
  readonly randomKey?: (length: number) => Uint8Array
}

const defaultEntryFactory: KeyringEntryFactory = async (service, account) => {
  const { AsyncEntry } = await import("@napi-rs/keyring")
  return new AsyncEntry(service, account)
}

const defaultRandomKey = (length: number): Uint8Array => {
  const key = new Uint8Array(length)
  nodeWebCrypto.getRandomValues(key)
  return key
}

const encodeHex = (value: Uint8Array): string => {
  const output = new Array<string>(value.byteLength)
  for (let index = 0; index < value.byteLength; index += 1) {
    output[index] = value[index]!.toString(16).padStart(2, "0")
  }
  return output.join("")
}

const decodeHex = (value: string, context: KeyContext): Uint8Array => {
  if (value.length !== CRYPTO_KEY_BYTES * 2 || !/^[0-9a-fA-F]+$/.test(value)) {
    throw makeCryptoError("invalid-key", "stored key is not a 64-character hex key", context)
  }
  const output = new Uint8Array(CRYPTO_KEY_BYTES)
  for (let index = 0; index < output.length; index += 1) {
    output[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16)
  }
  return output
}

const resolveNodeContext = (context: KeyContext): ResolvedKeyContext => resolveKeyContext(context)

/**
 * Resolve Rust-compatible 64-character hex keys from the OS credential store.
 *
 * Only `null`/`undefined` means that an entry is missing and may be generated.
 * Empty, malformed, or incorrectly sized values are corruption errors. A
 * provider error is propagated by the returned Effect and never causes a new
 * key to be generated. On Linux, the native package can fall back to kernel
 * keyutils; that cache is not durable across reboot, so Secret Service is
 * required when restart persistence is part of the deployment contract.
 * Windows DPAPI backup files are intentionally not implemented here, and this
 * provider never writes a plaintext backup.
 */
export const makeNodeKeyringProvider = (options: NodeKeyringProviderOptions = {}): KeyProvider => {
  const entryFactory = options.entryFactory ?? defaultEntryFactory
  const randomKey = options.randomKey ?? defaultRandomKey

  return {
    loadKey: async (context, signal) => {
      const resolved = resolveNodeContext(context)
      const entry = await entryFactory(resolved.service, resolved.account)
      const stored = await entry.getPassword(signal)
      if (stored != null) {
        return decodeHex(stored, context)
      }

      const generated = randomKey(CRYPTO_KEY_BYTES)
      if (generated.byteLength !== CRYPTO_KEY_BYTES) {
        throw makeCryptoError(
          "invalid-key",
          `generated crypto key must be exactly ${CRYPTO_KEY_BYTES} bytes`,
          context,
        )
      }
      // Do not expose the generated key until the keyring write has completed.
      await entry.setPassword(encodeHex(generated), signal)
      return new Uint8Array(generated)
    },
  }
}

/** Builds the Crypto service around one isolated Node keyring provider. */
export const makeNodeKeyringLayer = (
  options: NodeKeyringProviderOptions = {},
  cryptoOptions: CryptoLayerOptions = {},
): Layer.Layer<Crypto, never, never> => makeCryptoLayer(makeNodeKeyringProvider(options), cryptoOptions)

export const NodeKeyringLayer = makeNodeKeyringLayer
