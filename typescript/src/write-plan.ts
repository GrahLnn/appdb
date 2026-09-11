import { BoundQuery, RecordId } from "surrealdb"

import type { AnyModel } from "./model.js"
import { rawSql, type RawSql } from "./query.js"

/** The write branch whose SQL semantics are visible to the caller. */
export type WriteMode = "create" | "createAt" | "upsertAt" | "updateAt"

/** One result-bearing statement in a transaction query. */
export interface ResultSlot {
  readonly index: number
  readonly role: "parent" | "child" | "relation"
  readonly required: boolean
  readonly model: AnyModel
  readonly record: RecordId
  readonly inputIndex?: number
}

/**
 * A complete, parameterized transaction query and the result positions that
 * its consumer is allowed to decode. This is execution state, not a receipt or
 * a post-hoc validator.
 */
export interface WritePlan {
  readonly statement: RawSql
  readonly resultSlots: readonly ResultSlot[]
}

/** Mutable builder kept private to repository write orchestration. */
export class WritePlanBuilder {
  private statement: RawSql = rawSql("BEGIN TRANSACTION;")
  private nextIndex = 1
  private readonly resultSlots: ResultSlot[] = []

  appendStatement(
    sql: string,
    bindings: Readonly<Record<string, unknown>> = {},
  ): number {
    this.statement = this.statement.append(sql, { ...bindings })
    const index = this.nextIndex
    this.nextIndex += 1
    return index
  }

  appendBound(statement: RawSql): number {
    this.statement = this.statement.append(statement)
    const index = this.nextIndex
    this.nextIndex += 1
    return index
  }

  appendResult(
    sql: string,
    bindings: Readonly<Record<string, unknown>>,
    result: Omit<ResultSlot, "index">,
  ): void {
    const index = this.appendStatement(sql, bindings)
    this.resultSlots.push({ ...result, index })
  }

  appendResultBound(statement: RawSql, result: Omit<ResultSlot, "index">): void {
    const index = this.appendBound(statement)
    this.resultSlots.push({ ...result, index })
  }

  finish(): WritePlan {
    this.statement = new BoundQuery(this.statement).append("COMMIT TRANSACTION;")
    return {
      statement: this.statement,
      resultSlots: [...this.resultSlots],
    }
  }
}
