import Database from "better-sqlite3"
import type { SqliteClient, SqliteParam, SqliteRunResult } from "./sqlite.js"

const nativeParams = (params: readonly SqliteParam[] | undefined): readonly unknown[] =>
  (params ?? []).map((value) => value instanceof Uint8Array ? Buffer.from(value) : value)

/** A Node-owned SQLite connection and the appdb driver boundary. */
export interface NodeSqliteClient {
  readonly client: SqliteClient
  readonly database: Database.Database
  readonly close: () => void
}

/** Opens a synchronous SQLite connection for `makeSqliteStore`. */
export const openSqliteClient = (
  filename: string,
  options?: Database.Options,
): NodeSqliteClient => {
  const database = new Database(filename, options)
  const statements = new Map<string, Database.Statement>()
  const statement = (sql: string): Database.Statement => {
    const cached = statements.get(sql)
    if (cached !== undefined) return cached
    const prepared = database.prepare(sql)
    statements.set(sql, prepared)
    return prepared
  }
  const client: SqliteClient = {
    exec: (sql) => {
      database.exec(sql)
    },
    run: (sql, params): SqliteRunResult => {
      const result = statement(sql).run(...nativeParams(params))
      return { changes: Number(result.changes) }
    },
    query: <Row extends Record<string, unknown> = Record<string, unknown>>(sql: string, params?: readonly SqliteParam[]) =>
      statement(sql).all(...nativeParams(params)) as readonly Row[],
    transaction: async <A>(work: () => A | Promise<A>): Promise<A> => {
      database.exec("BEGIN")
      try {
        const result = await work()
        database.exec("COMMIT")
        return result
      } catch (error) {
        try {
          database.exec("ROLLBACK")
        } catch {
          // Preserve the original transaction failure.
        }
        throw error
      }
    },
  }
  return { client, database, close: () => database.close() }
}
