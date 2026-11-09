import { buildMessages, parseModelOutput, type ChatMessage, type Source } from "./prompt";
import type { ReserveResult } from "./usage";

export type { ChatMessage };

export const LIMITS = {
  question: 500,
  sources: 5,
  sourceText: 1000,
  perAccountPerDay: 20,
  globalPerDay: 200,
  maxTokens: 400,
} as const;

export interface AskDeps {
  verify(request: Request): Promise<string | null>;
  usage: {
    reserve(account: string, day: string, perAccount: number, global: number): Promise<ReserveResult>;
    refund(account: string, day: string): Promise<void>;
  };
  /** The model's raw text, or null when the call failed. */
  llm(messages: ChatMessage[], maxTokens: number): Promise<string | null>;
  now(): Date;
}

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

type Parsed = { question: string; sources: Source[] } | { status: 400 | 413; error: string };

function parse(body: unknown): Parsed {
  const bad = { status: 400 as const, error: "bad_request" };
  if (typeof body !== "object" || body === null) return bad;
  const { question, sources } = body as { question?: unknown; sources?: unknown };
  if (typeof question !== "string" || question.trim() === "") return bad;
  if (!Array.isArray(sources) || sources.length === 0) return bad;
  if (question.length > LIMITS.question) return { status: 413, error: "question_too_long" };
  if (sources.length > LIMITS.sources) return { status: 413, error: "too_many_sources" };
  const clean: Source[] = [];
  for (const s of sources) {
    if (typeof s !== "object" || s === null) return bad;
    const { id, file_name, text } = s as Record<string, unknown>;
    if (typeof id !== "string" || id === "" || id.length > 64) return bad;
    if (typeof file_name !== "string" || file_name.length > 255) return bad;
    if (typeof text !== "string") return bad;
    if (text.length > LIMITS.sourceText) return { status: 413, error: "source_too_long" };
    clean.push({ id, file_name, text });
  }
  return { question: question.trim(), sources: clean };
}

export async function handleAsk(request: Request, deps: AskDeps): Promise<Response> {
  const account = await deps.verify(request);
  if (!account) return json({ error: "unauthorized" }, 401);

  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return json({ error: "bad_request" }, 400);
  }
  const parsed = parse(body);
  if ("error" in parsed) return json({ error: parsed.error }, parsed.status);

  const day = deps.now().toISOString().slice(0, 10);
  const slot = await deps.usage.reserve(account, day, LIMITS.perAccountPerDay, LIMITS.globalPerDay);
  if (!slot.ok) {
    return json(
      { error: "limit", scope: slot.scope, used: slot.used, limit: LIMITS.perAccountPerDay },
      429,
    );
  }

  // From here on a failure gives the question back.
  try {
    const raw = await deps.llm(buildMessages(parsed.question, parsed.sources), LIMITS.maxTokens);
    const out = raw === null ? null : parseModelOutput(raw, parsed.sources.map((s) => s.id));
    if (out) {
      return json({ answer: out.answer, cited: out.cited, used: slot.used, limit: LIMITS.perAccountPerDay });
    }
  } catch {
    // fall through to the refund
  }
  await deps.usage.refund(account, day);
  return json({ error: "upstream" }, 502);
}
