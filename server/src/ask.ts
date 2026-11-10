import { buildMessages, parseModelOutput, type ChatMessage, type Image, type Source } from "./prompt";
import type { ReserveResult } from "./usage";

export type { ChatMessage };

export const LIMITS = {
  question: 500,
  // Files of one question, text and pictures together.
  sources: 5,
  // A file sent whole; passages stay far below (ask/context.rs).
  sourceText: 60_000,
  // All text together.
  totalText: 120_000,
  images: 3,
  // Base64 of one JPEG, 1024 px at most on its longer side (ask/image.rs).
  imageData: 2_000_000,
  perAccountPerDay: 20,
  globalPerDay: 200,
  maxTokens: 1200,
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

type Parsed =
  | { question: string; sources: Source[]; images: Image[] }
  | { status: 400 | 413; error: string };

// Only JPEG goes on to the model: base64 of FF D8 FF starts "/9j/".
const JPEG_BASE64 = /^\/9j\/[A-Za-z0-9+/]*={0,2}$/;

const validId = (id: unknown): id is string => typeof id === "string" && id !== "" && id.length <= 64;

function parse(body: unknown): Parsed {
  const bad = { status: 400 as const, error: "bad_request" };
  if (typeof body !== "object" || body === null) return bad;
  const { question, sources, images = [] } = body as {
    question?: unknown;
    sources?: unknown;
    images?: unknown;
  };
  if (typeof question !== "string" || question.trim() === "") return bad;
  if (!Array.isArray(sources) || !Array.isArray(images)) return bad;
  if (sources.length + images.length === 0) return bad;
  if (question.length > LIMITS.question) return { status: 413, error: "question_too_long" };
  if (sources.length + images.length > LIMITS.sources) return { status: 413, error: "too_many_sources" };
  if (images.length > LIMITS.images) return { status: 413, error: "too_many_images" };
  const clean: Source[] = [];
  let total = 0;
  for (const s of sources) {
    if (typeof s !== "object" || s === null) return bad;
    const { id, file_name, text } = s as Record<string, unknown>;
    if (!validId(id)) return bad;
    if (typeof file_name !== "string" || file_name.length > 255) return bad;
    if (typeof text !== "string") return bad;
    if (text.length > LIMITS.sourceText) return { status: 413, error: "source_too_long" };
    total += text.length;
    if (total > LIMITS.totalText) return { status: 413, error: "sources_too_long" };
    clean.push({ id, file_name, text });
  }
  const pictures: Image[] = [];
  for (const i of images) {
    if (typeof i !== "object" || i === null) return bad;
    const { id, file_name, data } = i as Record<string, unknown>;
    if (!validId(id)) return bad;
    if (typeof file_name !== "string" || file_name.length > 255) return bad;
    if (typeof data !== "string") return bad;
    if (data.length > LIMITS.imageData) return { status: 413, error: "image_too_large" };
    if (!JPEG_BASE64.test(data)) return bad;
    pictures.push({ id, file_name, data });
  }
  const ids = [...clean, ...pictures].map((s) => s.id);
  if (new Set(ids).size !== ids.length) return bad;
  return { question: question.trim(), sources: clean, images: pictures };
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
    const messages = buildMessages(parsed.question, parsed.sources, parsed.images);
    const raw = await deps.llm(messages, LIMITS.maxTokens);
    const sent = [...parsed.sources, ...parsed.images].map((s) => s.id);
    const out = raw === null ? null : parseModelOutput(raw, sent);
    if (out) {
      return json({ answer: out.answer, cited: out.cited, used: slot.used, limit: LIMITS.perAccountPerDay });
    }
  } catch {
    // fall through to the refund
  }
  await deps.usage.refund(account, day);
  return json({ error: "upstream" }, 502);
}
