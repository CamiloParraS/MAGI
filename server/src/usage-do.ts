import { DurableObject } from "cloudflare:workers";
import { UsageStore, type ReserveResult } from "./usage";

// One instance holds every count. Methods are synchronous, so each call is atomic.
export class Usage extends DurableObject {
  private store: UsageStore;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    this.store = new UsageStore(ctx.storage.sql);
  }

  reserve(account: string, day: string, perAccount: number, global: number): ReserveResult {
    return this.store.reserve(account, day, perAccount, global);
  }

  refund(account: string, day: string): void {
    this.store.refund(account, day);
  }

  deleteAccount(account: string): void {
    this.store.deleteAccount(account);
  }
}
