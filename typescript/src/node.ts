import { createNodeEngines } from "@surrealdb/node"
import { createRemoteEngines, Surreal, type DriverOptions } from "surrealdb"
import type { Layer } from "effect"
import {
  makeDatabaseLayer,
  type DatabaseLayerOptions,
} from "./connection.js"
import type { Database } from "./connection.js"
import type { DbError } from "./errors.js"

export type NodeEmbeddedBackend = "mem" | "rocksdb" | "surrealkv"
export type NodeEngineOptions = Parameters<typeof createNodeEngines>[0]

export interface NodeEmbeddedEndpoint {
  readonly backend: NodeEmbeddedBackend
  /** Required for every on-disk backend; `mem` has no implicit path. */
  readonly path?: string
}

export type NodeEndpoint = string | URL | NodeEmbeddedEndpoint

export interface NodeDatabaseLayerOptions
  extends Omit<DatabaseLayerOptions, "makeClient" | "clientOptions" | "endpoint" | "connectionErrorKind"> {
  /** An explicit `mem://`, `rocksdb://`, `surrealkv://`, or remote endpoint. */
  readonly endpoint: NodeEndpoint
  readonly engineOptions?: NodeEngineOptions
  readonly clientOptions?: Omit<DriverOptions, "engines">
}

// A drive in URL authority loses its colon in WHATWG URL normalization.
// The native engine accepts an opaque URI, which preserves the filesystem path.
const preserveNativeDrive = (endpoint: string): string =>
  endpoint.replace(/^(surrealkv|rocksdb):\/\/([a-z]:\/)/i, "$1:$2")

const toEndpoint = (endpoint: NodeEndpoint): string | URL => {
  if (typeof endpoint === "string") return preserveNativeDrive(endpoint)
  if (endpoint instanceof URL) return endpoint

  if (endpoint.backend === "mem") {
    if (endpoint.path !== undefined) {
      throw new TypeError("mem backend does not accept a storage path")
    }
    return "mem://"
  }

  if (endpoint.path === undefined || endpoint.path.length === 0) {
    throw new TypeError(`${endpoint.backend} backend requires an explicit storage path`)
  }

  return preserveNativeDrive(`${endpoint.backend}://${endpoint.path.replaceAll("\\", "/")}`)
}

/** Construct an embedded endpoint without selecting a storage backend implicitly. */
export const nodeEndpoint = (
  backend: NodeEmbeddedBackend,
  path?: string,
): NodeEmbeddedEndpoint => ({
  backend,
  ...(path === undefined ? {} : { path }),
})

const makeNodeClient = (
  engineOptions: NodeEngineOptions | undefined,
  clientOptions: Omit<DriverOptions, "engines"> | undefined,
): Surreal =>
  new Surreal({
    ...(clientOptions ?? {}),
    engines: {
      ...createRemoteEngines(),
      ...createNodeEngines(engineOptions),
    },
  })

/**
 * Build the Node.js layer. Native engines are imported only from this entry;
 * the shared connection layer remains usable with remote engines alone.
 */
export const makeNodeDatabaseLayer = (
  options: NodeDatabaseLayerOptions,
): Layer.Layer<Database, DbError, never> =>
  makeDatabaseLayer({
    ...options,
    endpoint: toEndpoint(options.endpoint),
    connectionErrorKind: "Engine",
    makeClient: () => makeNodeClient(options.engineOptions, options.clientOptions),
  })

export const NodeDatabaseLayer = makeNodeDatabaseLayer
