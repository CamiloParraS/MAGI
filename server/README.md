# Magi Ask server

Cloudflare Worker behind Ask Magi. Design: `docs/issues/ask-magi/plan.md`,
`docs/adr/0014-optional-online-ask-magi.md`. Never log bodies; invocation logs
stay off (`wrangler.toml`).

- `POST /ask` verifies the Supabase token, enforces size caps and the daily
  limits (20 per account, 200 overall, UTC day), calls OpenRouter on
  zero-retention hosts, returns `{ answer, cited, used, limit }`. A failed call
  is not counted.
- `DELETE /account` deletes the Supabase user and the usage row.
- A daily cron reads the `keepalive` table so the free Supabase project does
  not pause.

## Develop

```
npm install
npm test          # vitest, no network
npm run typecheck
```

## First deploy

1. Supabase SQL editor, once:
   ```sql
   create table keepalive (id int primary key);
   insert into keepalive values (1);
   alter table keepalive enable row level security;
   ```
2. Secrets (paste the value at the prompt, never in chat or git):
   ```
   npx wrangler secret put OPENROUTER_API_KEY
   npx wrangler secret put SUPABASE_SERVICE_KEY   # the secret / service_role key
   ```
3. `npx wrangler deploy`

## Runbook

- **Rotate a key:** create the new one (OpenRouter Keys, or Supabase Project
  Settings, API Keys), run `npx wrangler secret put <NAME>`, then delete the old
  one. No redeploy needed.
- **Supabase project paused:** dashboard, Restore project. Wait for it to come
  up, then check `POST /ask` with a fresh sign-in. Sessions survive a restore.
- **Top up or check credit:** openrouter.ai, Settings, Credits. Activity shows
  cost per request. A question with passages costs roughly $0.0003; one with
  whole files at the 120,000-character cap about $0.004, so 200 a day costs at
  most about $0.80.
- **See that the cron ran:** Cloudflare dashboard, Workers, magi-ask, Settings,
  Triggers (cron), or `npx wrangler tail` while it fires. A failure means the
  Supabase call returned an error.
- **Change a limit:** `LIMITS` in `src/ask.ts`, then deploy.
- **Change the model or the `provider` routing:** `MODEL` in `wrangler.toml`
  or `llm()` in `src/index.ts`. The model must take images, and OpenRouter
  must list zero-retention hosts for it that support `response_format`
  (`/api/v1/models/<model>/endpoints`). Deploy, then run the live check from
  the repo root: `cargo test -p magi-core --test ask_live -- --ignored
  --nocapture`. Only this catches a model or host that the unit tests cannot.
- **A question failed (502):** the body's `reason` says why
  (`model_status_<n>: <OpenRouter's message>`, `model_timeout`,
  `model_unreachable`, `reply_empty`, `reply_not_json`, `worker_error`). The app
  writes it to its own log as "Ask Magi got no answer".
- **Logs:** keep `[observability]` disabled. Do not add `console` calls.
