/**
 * Node-only appdb API.
 *
 * Keeping this as a separate entry prevents browser and other cross-platform
 * consumers from loading native SurrealDB engines or OS credential-store code.
 */
export * from "./node.js"
export * from "./node-keyring.js"
