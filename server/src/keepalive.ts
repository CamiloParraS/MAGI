// Free Supabase projects pause after 7 idle days. A daily read keeps one awake.
// Needs a one-row `keepalive` table (see server/README.md).

// New `sb_secret_...` keys are not JWTs, so they go in `apikey` only. The legacy
// service_role key is a JWT and also goes in `Authorization`.
export function serviceHeaders(key: string): Record<string, string> {
  return key.startsWith("sb_") ? { apikey: key } : { apikey: key, authorization: `Bearer ${key}` };
}

export async function keepAlive(
  env: { SUPABASE_URL: string; SUPABASE_SERVICE_KEY: string },
  fetcher: (url: string, init: RequestInit) => Promise<Response> = fetch,
): Promise<void> {
  const res = await fetcher(`${env.SUPABASE_URL}/rest/v1/keepalive?select=id&limit=1`, {
    headers: serviceHeaders(env.SUPABASE_SERVICE_KEY),
  });
  if (!res.ok) throw new Error(`keep-alive failed: ${res.status}`);
}
