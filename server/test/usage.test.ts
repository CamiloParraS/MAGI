import { describe, expect, it } from "vitest";
import { memSql, memStore } from "./helpers";
import { UsageStore } from "../src/usage";

describe("UsageStore", () => {
  it("counts per account and stops at the account limit", () => {
    const s = memStore();
    for (let i = 1; i <= 20; i++) {
      expect(s.reserve("a", "2026-11-10", 20, 200)).toEqual({ ok: true, used: i });
    }
    expect(s.reserve("a", "2026-11-10", 20, 200)).toEqual({ ok: false, scope: "account", used: 20 });
    expect(s.reserve("b", "2026-11-10", 20, 200)).toEqual({ ok: true, used: 1 });
  });

  it("stops at the global limit", () => {
    const s = memStore();
    for (let i = 0; i < 10; i++) for (let j = 0; j < 20; j++) s.reserve(`u${i}`, "2026-11-10", 20, 200);
    expect(s.reserve("new", "2026-11-10", 20, 200)).toMatchObject({ ok: false, scope: "global" });
  });

  it("starts a new count on a new UTC day and drops old rows", () => {
    const sql = memSql();
    const s = new UsageStore(sql);
    s.reserve("a", "2026-11-10", 20, 200);
    expect(s.reserve("a", "2026-11-11", 20, 200)).toEqual({ ok: true, used: 1 });
    const days = sql.exec("SELECT day FROM usage").toArray().map((r) => r.day);
    expect(days).toEqual(["2026-11-11"]);
  });

  it("refunds one, never below zero", () => {
    const s = memStore();
    s.reserve("a", "2026-11-10", 20, 200);
    s.refund("a", "2026-11-10");
    s.refund("a", "2026-11-10");
    expect(s.reserve("a", "2026-11-10", 20, 200)).toEqual({ ok: true, used: 1 });
  });

  it("deletes an account's rows", () => {
    const s = memStore();
    s.reserve("a", "2026-11-10", 20, 200);
    s.deleteAccount("a");
    expect(s.reserve("a", "2026-11-10", 20, 200)).toEqual({ ok: true, used: 1 });
  });

  it("stores only account id, day and count", () => {
    const sql = memSql();
    new UsageStore(sql);
    const cols = sql.exec("PRAGMA table_info(usage)").toArray().map((r) => r.name);
    expect(cols).toEqual(["account", "day", "count"]);
  });
});
