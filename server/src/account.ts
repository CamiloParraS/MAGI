import { json } from "./ask";

export interface AccountDeps {
  verify(request: Request): Promise<string | null>;
  /** Deletes the Supabase user. True when it is gone. */
  deleteUser(id: string): Promise<boolean>;
  deleteUsage(id: string): Promise<void>;
}

export async function handleDeleteAccount(request: Request, deps: AccountDeps): Promise<Response> {
  const id = await deps.verify(request);
  if (!id) return json({ error: "unauthorized" }, 401);
  if (!(await deps.deleteUser(id))) return json({ error: "upstream" }, 502);
  await deps.deleteUsage(id);
  return new Response(null, { status: 204 });
}
