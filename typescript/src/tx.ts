import { Data, Effect } from "effect"
import { BoundQuery, type QueryResponse, type SurrealTransaction } from "surrealdb"

import { Database, type DatabaseApi, type DatabaseResponse } from "./connection.js"
import { dbErrorFromCause, DbError, DbErrorKind } from "./errors.js"
import { selectResponseFailure } from "./query.js"

/** One input transaction item. Its SQL may contain any number of statements. */
export class TxStmt<R extends unknown[] = unknown[]> {
  readonly statement: BoundQuery<R>

  constructor(statement: BoundQuery<R> | string, bindings: Readonly<Record<string, unknown>> = {}) {
    if (typeof statement === "string") {
      this.statement = new BoundQuery<R>(statement, { ...bindings })
    } else {
      this.statement = Object.keys(bindings).length === 0
        ? new BoundQuery<R>(statement)
        : new BoundQuery<R>(statement.query, { ...statement.bindings, ...bindings })
    }
  }

  /** Rust `TxStmt::bind` replaces a same-name binding rather than appending a second one. */
  bind<K extends string, V>(key: K, value: V): TxStmt<R> {
    return new TxStmt<R>(this.statement.query, {
      ...this.statement.bindings,
      [key]: value,
    })
  }

  get query(): BoundQuery<R> {
    return new BoundQuery<R>(this.statement)
  }

  get sql(): string {
    return this.statement.query
  }

  get bindings(): Readonly<Record<string, unknown>> {
    return this.statement.bindings
  }
}

export type TxInput = TxStmt | BoundQuery | string
export type TxResponse = QueryResponse<unknown>

/** A single input TxStmt's response slots, preserving multi-statement grouping. */
export class TxStatementResults {
  readonly responses: readonly TxResponse[]

  constructor(responses: readonly TxResponse[]) {
    this.responses = [...responses]
  }

  get length(): number {
    return this.responses.length
  }

  get(index: number): TxResponse | undefined {
    return this.responses[index]
  }

  take<R = unknown>(index = 0): R {
    const response = this.responses[index]
    if (response === undefined) {
      throw new DbError({
        kind: DbErrorKind.EmptyResult,
        message: `transaction result slot ${index} is missing`,
        operation: "runTx",
      })
    }
    if (!response.success) {
      throw responseError(response, index)
    }
    return response.result as R
  }

  toArray(): readonly TxResponse[] {
    return [...this.responses]
  }
}

/** Grouped transaction output corresponding one-to-one with the input TxStmt list. */
export class TxResults {
  readonly statements: readonly TxStatementResults[]

  constructor(groups: readonly (readonly TxResponse[])[]) {
    this.statements = groups.map((group) => new TxStatementResults(group))
  }

  len(): number {
    return this.statements.length
  }

  isEmpty(): boolean {
    return this.statements.length === 0
  }

  get(index: number): TxStatementResults | undefined {
    return this.statements[index]
  }

  take<R = unknown>(statementIndex: number, resultIndex = 0): R {
    const statement = this.statements[statementIndex]
    if (statement === undefined) {
      throw new DbError({
        kind: DbErrorKind.EmptyResult,
        message: `transaction statement index ${statementIndex} is out of range`,
        operation: "runTx",
      })
    }
    return statement.take<R>(resultIndex)
  }

  intoInner(): readonly (readonly TxResponse[])[] {
    return this.statements.map((statement) => statement.toArray())
  }
}

/** Typed error emitted before execution when the selected engine has no transactions. */
export class TransactionCapabilityError extends Data.TaggedError("TransactionCapabilityError")<{
  readonly capability: "transactions"
  readonly message: string
  readonly operation: "runTx"
  readonly cause?: unknown
}> {}

export type TxError = DbError | TransactionCapabilityError

export const isTransactionCapabilityError = (value: unknown): value is TransactionCapabilityError =>
  typeof value === "object" && value !== null && "_tag" in value && value._tag === "TransactionCapabilityError"

const inputQuery = (input: TxInput): BoundQuery => {
  if (input instanceof TxStmt) return input.statement
  if (input instanceof BoundQuery) return input
  return new BoundQuery(input)
}

const isCapabilityFailure = (cause: unknown): boolean => {
  if (typeof cause !== "object" || cause === null) return false
  const value = cause as { name?: unknown; message?: unknown; kind?: unknown }
  const name = typeof value.name === "string" ? value.name : ""
  const message = typeof value.message === "string" ? value.message : ""
  const kind = typeof value.kind === "string" ? value.kind : ""
  return name === "UnsupportedFeatureError" ||
    name === "UnavailableFeatureError" ||
    kind === "UnsupportedFeature" ||
    /unsupported feature.*transaction|transaction.*not supported/i.test(`${name} ${message} ${kind}`)
}

const capabilityError = (cause: unknown): TransactionCapabilityError =>
  new TransactionCapabilityError({
    capability: "transactions",
    operation: "runTx",
    message: "the selected SurrealDB engine does not support transactions",
    cause,
  })

const txDbError = (operation: string, cause: unknown): DbError => {
  return dbErrorFromCause(operation, cause)
}

const responseError = (response: Extract<TxResponse, { success: false }>, resultIndex: number): DbError =>
  txDbError(`runTx result ${resultIndex}`, response.error)

const checkResponses = (
  responses: readonly DatabaseResponse[],
  inputIndex: number,
): Effect.Effect<readonly TxResponse[], DbError> =>
  Effect.suspend(() => {
    const failure = selectResponseFailure(responses)
    if (failure !== undefined) {
      return Effect.fail(
        txDbError(`runTx statement ${inputIndex} result ${failure.index}`, failure.response.error),
      )
    }
    return Effect.succeed(responses)
  })

const beginTransaction = (database: DatabaseApi): Effect.Effect<SurrealTransaction, TxError> =>
  Effect.uninterruptible(
    Effect.tryPromise({
      try: () => {
        const begin = database.client.beginTransaction
        if (typeof begin !== "function") {
          throw capabilityError(new Error("client.beginTransaction is unavailable"))
        }
        return begin.call(database.client)
      },
      catch: (cause) => isTransactionCapabilityError(cause) || isCapabilityFailure(cause)
        ? isTransactionCapabilityError(cause) ? cause : capabilityError(cause)
        : txDbError("runTx begin", cause),
    }),
  )

const cancelTransaction = (transaction: SurrealTransaction): Effect.Effect<void, never> =>
  Effect.uninterruptible(
    Effect.tryPromise({
      try: () => transaction.cancel(),
      catch: () => undefined,
    }).pipe(Effect.asVoid, Effect.catchCause(() => Effect.succeed(undefined))),
  )

const runTransaction = (
  database: DatabaseApi,
  inputs: readonly TxInput[],
): Effect.Effect<TxResults, TxError> =>
  Effect.gen(function* () {
    const transaction = yield* beginTransaction(database)
    let committed = false
    const work = Effect.gen(function* () {
      const groups: Array<readonly TxResponse[]> = []
      for (const [inputIndex, input] of inputs.entries()) {
        const responses = yield* Effect.uninterruptible(
          Effect.tryPromise({
            try: () => transaction.query(inputQuery(input)).responses(),
            catch: (cause) => isCapabilityFailure(cause)
              ? capabilityError(cause)
              : txDbError(`runTx query ${inputIndex}`, cause),
          }),
        )
        groups.push(yield* checkResponses(responses, inputIndex))
      }
      // A successful commit and the local `committed` publication are one
      // state transition. Mask interruption until both have completed so the
      // finalizer cannot cancel a transaction that has already committed.
      yield* Effect.uninterruptible(
        Effect.tryPromise({
          try: () => transaction.commit(),
          catch: (cause) => isCapabilityFailure(cause)
            ? capabilityError(cause)
            : txDbError("runTx commit", cause),
        }).pipe(
          Effect.tap(() => Effect.sync(() => {
            committed = true
          })),
        ),
      )

      // Rust's empty-input behavior commits the transaction first, then runs
      // `RETURN NONE` outside the transaction. Retain its result as one group.
      if (inputs.length === 0) {
        const responses = yield* database.queryUnchecked("RETURN NONE;")
        groups.push(yield* checkResponses(responses, 0))
      }
      return new TxResults(groups)
    })
    return yield* work.pipe(
      Effect.ensuring(
        Effect.suspend(() => committed ? Effect.succeed(undefined) : cancelTransaction(transaction)),
      ),
    )
  })

/** Execute input transaction items atomically, retaining one response group per input item. */
export const runTx = (
  inputs: readonly TxInput[],
): Effect.Effect<TxResults, TxError, Database> =>
  Effect.gen(function* () {
    const database = yield* Database
    return yield* runTransaction(database, inputs)
  })

export const tx = runTx
