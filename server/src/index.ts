import { createRemoteJWKSet, jwtVerify } from "jose";
import { handleDeleteAccount } from "./account";
import { handleAsk, json, type ChatMessage } from "./ask";
import { keepAlive, serviceHeaders } from "./keepalive";
import type { Usage } from "./usage-do";

export { Usage } from "./usage-do";

export interface Env {
  SUPABASE_URL: string;
  SUPABASE_SERVICE_KEY: string;
  OPENROUTER_API_KEY: string;
  MODEL: string;
  USAGE: DurableObjectNamespace<Usage>;
}

// No console output anywhere: bodies must never reach a log.

let jwks: ReturnType<typeof createRemoteJWKSet> | undefined;

async function verify(request: Request, env: Env): Promise<string | null> {
  const header = request.headers.get("authorization") ?? "";
  const token = header.startsWith("Bearer ") ? header.slice(7) : "";
  if (!token) return null;
  const base = `${env.SUPABASE_URL}/auth/v1`;
  jwks ??= createRemoteJWKSet(new URL(`${base}/.well-known/jwks.json`));
  try {
    const { payload } = await jwtVerify(token, jwks, { issuer: base, audience: "authenticated" });
    return typeof payload.sub === "string" ? payload.sub : null;
  } catch {
    return null;
  }
}

async function llm(env: Env, messages: ChatMessage[], maxTokens: number): Promise<string | null> {
  try {
    const res = await fetch("https://openrouter.ai/api/v1/chat/completions", {
      method: "POST",
      headers: { authorization: `Bearer ${env.OPENROUTER_API_KEY}`, "content-type": "application/json" },
      signal: AbortSignal.timeout(25_000),
      body: JSON.stringify({
        model: env.MODEL,
        max_tokens: maxTokens,
        response_format: { type: "json_object" },
        provider: { zdr: true, data_collection: "deny" },
        messages,
      }),
    });
    if (!res.ok) return null;
    const data = (await res.json()) as { choices?: { message?: { content?: string } }[] };
    return data.choices?.[0]?.message?.content ?? null;
  } catch {
    return null;
  }
}

async function deleteUser(env: Env, id: string): Promise<boolean> {
  try {
    const res = await fetch(`${env.SUPABASE_URL}/auth/v1/admin/users/${encodeURIComponent(id)}`, {
      method: "DELETE",
      headers: serviceHeaders(env.SUPABASE_SERVICE_KEY),
    });
    return res.ok || res.status === 404;
  } catch {
    return false;
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    // US placement for the one Durable Object that holds the counts.
    const usage = env.USAGE.get(env.USAGE.idFromName("global"), { locationHint: "enam" });
    const verifyReq = (r: Request) => verify(r, env);

    if (request.method === "POST" && pathname === "/ask") {
      return handleAsk(request, {
        verify: verifyReq,
        usage: {
          reserve: (a, d, pa, g) => usage.reserve(a, d, pa, g),
          refund: (a, d) => usage.refund(a, d),
        },
        llm: (m, t) => llm(env, m, t),
        now: () => new Date(),
      });
    }
    if (request.method === "DELETE" && pathname === "/account") {
      return handleDeleteAccount(request, {
        verify: verifyReq,
        deleteUser: (id) => deleteUser(env, id),
        deleteUsage: (id) => usage.deleteAccount(id),
      });
    }
    return json({ error: "not_found" }, 404);
  },

  async scheduled(_event: ScheduledController, env: Env): Promise<void> {
    await keepAlive(env);
  },
};
