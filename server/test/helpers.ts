import { DatabaseSync } from "node:sqlite";
import { UsageStore, type SqlLike } from "../src/usage";

// The Durable Object's SQL cursor, backed by node:sqlite so the real queries run.
export function memSql(): SqlLike {
  const db = new DatabaseSync(":memory:");
  return {
    exec(query: string, ...bindings: unknown[]) {
      const stmt = db.prepare(query);
      if (/^\s*(select|pragma)/i.test(query)) {
        return { toArray: () => stmt.all(...(bindings as never[])) as Record<string, unknown>[] };
      }
      stmt.run(...(bindings as never[]));
      return { toArray: () => [] };
    },
  };
}

export function memStore(): UsageStore {
  return new UsageStore(memSql());
}
