// Daily question counts. Stores only account id, UTC day and count.
// Plain SQL over a tiny interface so tests run it on node:sqlite and the
// Durable Object runs it on ctx.storage.sql.

export interface SqlLike {
  exec(query: string, ...bindings: unknown[]): { toArray(): Record<string, unknown>[] };
}

export type ReserveResult =
  | { ok: true; used: number }
  | { ok: false; scope: "account" | "global"; used: number };

export class UsageStore {
  constructor(private sql: SqlLike) {
    sql.exec(
      "CREATE TABLE IF NOT EXISTS usage (account TEXT NOT NULL, day TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY (account, day))",
    );
  }

  private num(query: string, ...bindings: unknown[]): number {
    const row = this.sql.exec(query, ...bindings).toArray()[0];
    return row ? Number(row.n) : 0;
  }

  /** Counts one question if both limits allow it. Synchronous, so atomic in a Durable Object. */
  reserve(account: string, day: string, perAccount: number, global: number): ReserveResult {
    this.sql.exec("DELETE FROM usage WHERE day < ?", day);
    const mine = this.num("SELECT count AS n FROM usage WHERE account = ? AND day = ?", account, day);
    if (mine >= perAccount) return { ok: false, scope: "account", used: mine };
    const all = this.num("SELECT COALESCE(SUM(count), 0) AS n FROM usage WHERE day = ?", day);
    if (all >= global) return { ok: false, scope: "global", used: mine };
    this.sql.exec(
      "INSERT INTO usage (account, day, count) VALUES (?, ?, 1) ON CONFLICT (account, day) DO UPDATE SET count = count + 1",
      account,
      day,
    );
    return { ok: true, used: mine + 1 };
  }

  refund(account: string, day: string): void {
    this.sql.exec("UPDATE usage SET count = count - 1 WHERE account = ? AND day = ? AND count > 0", account, day);
  }

  deleteAccount(account: string): void {
    this.sql.exec("DELETE FROM usage WHERE account = ?", account);
  }
}
