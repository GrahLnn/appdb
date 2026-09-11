import { defineConfig } from "vitest/config"

export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts", "tests/**/*.test.ts"],
    pool: "forks"
  }
})
