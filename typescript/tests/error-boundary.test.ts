import { Effect, Exit, Option } from "effect"
import { describe, expect, it } from "vitest"

import { type DatabaseResponse } from "../src/connection.js"
import {
  classifyErrorKind,
  dbErrorFromCause,
  DbErrorKind,
  isRollbackPlaceholder,
} from "../src/errors.js"
import { checkedResponseSlots, selectResponseFailure } from "../src/query.js"

const response = (value: unknown): DatabaseResponse => value as DatabaseResponse

const duplicateError = {
  name: "AlreadyExistsError",
  kind: "AlreadyExists",
  message: "Database record `item:one` already exists",
  details: { kind: "Record", details: { id: "item:one" } },
}

const skippedError = {
  name: "QueryError",
  kind: "Query",
  message: "The query was not executed due to a failed transaction",
  details: { kind: "NotExecuted" },
}

describe("native database error boundary", () => {
  it("maps duplicate SDK errors to the shared conflict category", () => {
    expect(classifyErrorKind(duplicateError)).toBe(DbErrorKind.Conflict)
    expect(dbErrorFromCause("write", duplicateError)).toMatchObject({
      kind: DbErrorKind.Conflict,
      operation: "write",
    })
  })

  it("selects the real response cause while retaining slot order", async () => {
    const responses = [
      response({ success: false, error: skippedError }),
      response({ success: false, error: duplicateError }),
      response({ success: true, result: undefined }),
    ]

    expect(isRollbackPlaceholder(skippedError)).toBe(true)
    expect(selectResponseFailure(responses)).toMatchObject({
      index: 1,
      response: { error: duplicateError },
    })

    const failure = await Effect.runPromiseExit(checkedResponseSlots(responses, "write"))
    expect(Exit.isFailure(failure)).toBe(true)
    expect(Option.getOrUndefined(Exit.findErrorOption(failure))).toMatchObject({
      kind: DbErrorKind.Conflict,
      operation: "write result 1",
    })

    const successful = await Effect.runPromise(
      checkedResponseSlots([
        response({ success: true, result: undefined }),
        response({ success: true, result: [{ id: "one" }] }),
      ], "write"),
    )
    expect(successful).toEqual([undefined, [{ id: "one" }]])
  })
})
