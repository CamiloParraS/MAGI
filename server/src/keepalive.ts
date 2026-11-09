// Free Supabase projects pause after 7 idle days. A daily read keeps one awake.
// Needs a one-row `keepalive` table (see server/README.md).

export async function keepAlive(
  env: { SUPABASE_URL: string; SUPABASE_SERVICE_KEY: string },
  fetcher: (url: string, init: RequestInit) => Promise<Response> = fetch,
): Promise<void> {
  const res = await fetcher(`${env.SUPABASE_URL}/rest/v1/keepalive?select=id&limit=1`, {
    headers: { apikey: env.SUPABASE_SERVICE_KEY, authorization: `Bearer ${env.SUPABASE_SERVICE_KEY}` },
  });
  if (!res.ok) throw new Error(`keep-alive failed: ${res.status}`);
}
