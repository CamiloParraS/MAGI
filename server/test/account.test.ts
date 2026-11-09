import { describe, expect, it, vi } from "vitest";
import { handleDeleteAccount } from "../src/account";
import { keepAlive } from "../src/keepalive";

const req = () => new Request("https://x/account", { method: "DELETE" });

describe("handleDeleteAccount", () => {
  it("requires a verified caller", async () => {
    const deleteUser = vi.fn();
    const res = await handleDeleteAccount(req(), {
      verify: async () => null,
      deleteUser,
      deleteUsage: vi.fn(),
    });
    expect(res.status).toBe(401);
    expect(deleteUser).not.toHaveBeenCalled();
  });

  it("deletes the user, then the usage row", async () => {
    const order: string[] = [];
    const res = await handleDeleteAccount(req(), {
      verify: async () => "acct-1",
      deleteUser: async (id) => {
        order.push(`user:${id}`);
        return true;
      },
      deleteUsage: async (id) => {
        order.push(`usage:${id}`);
      },
    });
    expect(res.status).toBe(204);
    expect(order).toEqual(["user:acct-1", "usage:acct-1"]);
  });

  it("keeps the usage row when the user delete fails", async () => {
    const deleteUsage = vi.fn();
    const res = await handleDeleteAccount(req(), {
      verify: async () => "acct-1",
      deleteUser: async () => false,
      deleteUsage,
    });
    expect(res.status).toBe(502);
    expect(deleteUsage).not.toHaveBeenCalled();
  });
});

describe("keepAlive", () => {
  const env = { SUPABASE_URL: "https://p.supabase.co", SUPABASE_SERVICE_KEY: "k" };

  it("reads the one-row table with the service key", async () => {
    const f = vi.fn(async (_url: string, _init: RequestInit) => new Response("[]", { status: 200 }));
    await keepAlive(env, f);
    const [url, init] = f.mock.calls[0];
    expect(url).toBe("https://p.supabase.co/rest/v1/keepalive?select=id&limit=1");
    expect((init.headers as Record<string, string>).apikey).toBe("k");
  });

  it("throws when Supabase answers an error, so the failure shows", async () => {
    const f = async () => new Response("x", { status: 503 });
    await expect(keepAlive(env, f)).rejects.toThrow();
  });
});
