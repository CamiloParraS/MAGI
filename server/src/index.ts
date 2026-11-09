import { createRemoteJWKSet, jwtVerify } from "jose";

export interface Env {
  SUPABASE_URL: string;
  MODEL: string;
  OPENROUTER_API_KEY: string;
}

// Day-1 spike: verify a Supabase token, make one zero-retention model call.
// Limits, size caps, refunds and the final response shape come on Nov 10.

let jwks: ReturnType<typeof createRemoteJWKSet> | undefined;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function verify(request: Request, env: Env): Promise<string | null> {
  const header = request.headers.get("authorization") ?? "";
  const token = header.startsWith("Bearer ") ? header.slice(7) : "";
  if (!token) return null;
  const base = `${env.SUPABASE_URL}/auth/v1`;
  jwks ??= createRemoteJWKSet(new URL(`${base}/.well-known/jwks.json`));
  try {
    const { payload } = await jwtVerify(token, jwks, {
      issuer: base,
      audience: "authenticated",
    });
    return typeof payload.sub === "string" ? payload.sub : null;
  } catch {
    return null;
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method !== "POST" || url.pathname !== "/ask") {
      return json({ error: "not_found" }, 404);
    }
    const account = await verify(request, env);
    if (!account) return json({ error: "unauthorized" }, 401);

    let body: { question?: string };
    try {
      body = await request.json();
    } catch {
      return json({ error: "bad_request" }, 400);
    }

    const upstream = await fetch("https://openrouter.ai/api/v1/chat/completions", {
      method: "POST",
      headers: {
        authorization: `Bearer ${env.OPENROUTER_API_KEY}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: env.MODEL,
        max_tokens: 300,
        response_format: { type: "json_object" },
        provider: { zdr: true, data_collection: "deny" },
        messages: [
          { role: "system", content: 'Reply with JSON: {"answer": string}.' },
          { role: "user", content: String(body.question ?? "").slice(0, 500) },
        ],
      }),
    });
    if (!upstream.ok) return json({ error: "upstream", status: upstream.status }, 502);

    const data = (await upstream.json()) as {
      choices?: { message?: { content?: string } }[];
      usage?: unknown;
    };
    // `usage` is returned only for the day-1 cost check; drop it on Nov 10.
    return json({
      account,
      content: data.choices?.[0]?.message?.content ?? null,
      usage: data.usage ?? null,
    });
  },
};
