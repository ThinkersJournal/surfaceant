import { DurableObject } from "cloudflare:workers";
import { b64, b64url, intVar, sha256, timingSafeEqual } from "./util";

const DROP_EXACT = new Set([
  "host", "connection", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade", "content-length",
  "accept-encoding", "x-real-ip", "true-client-ip", "cdn-loop",
]);
const DROP_PREFIX = ["cf-", "x-forwarded-", "proxy-", "x-envoy-"];

/** Pass everything through except hop-by-hop and Cloudflare-added headers. Order and duplicates are kept. */
export function passHeaders(h: Headers): [string, string][] {
  const out: [string, string][] = [];
  h.forEach((v, k) => {
    if (DROP_EXACT.has(k) || DROP_PREFIX.some((p) => k.startsWith(p))) return;
    out.push([k, v]);
  });
  return out;
}

interface Attachment {
  hello: boolean;
}

type Row = Record<string, SqlStorageValue>;

export class Channel extends DurableObject<Env> {
  private tokens: number | null = null;
  private refilled = 0;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      const sql = ctx.storage.sql;
      sql.exec("CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
      sql.exec(`CREATE TABLE IF NOT EXISTS events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT, delivery_id TEXT NOT NULL UNIQUE, received_at INTEGER NOT NULL,
        method TEXT NOT NULL, path_suffix TEXT NOT NULL, query TEXT NOT NULL, headers TEXT NOT NULL,
        body BLOB NOT NULL, size INTEGER NOT NULL)`);
    });
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  private meta(k: string): string | null {
    const r = this.ctx.storage.sql.exec("SELECT v FROM meta WHERE k = ?", k).toArray();
    return r.length ? (r[0].v as string) : null;
  }

  private setMeta(k: string, v: string) {
    this.ctx.storage.sql.exec("INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v", k, v);
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.hostname === "do") {
      if (url.pathname === "/init") {
        if (this.meta("token_hash") !== null) return new Response(null, { status: 409 });
        this.setMeta("token_hash", await request.text());
        return new Response(null, { status: 201 });
      }
      if (url.pathname === "/disable") {
        this.setMeta("disabled", "1");
        for (const ws of this.ctx.getWebSockets()) {
          this.sendTo(ws, { type: "error", code: "disabled", message: "channel disabled" });
          ws.close(1008, "disabled");
        }
        return new Response(null, { status: 204 });
      }
      return new Response(null, { status: 404 });
    }
    if (this.meta("token_hash") === null) return new Response(null, { status: 404 });
    if (this.meta("disabled") === "1") return new Response(null, { status: 410 });
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] === "connect") return this.upgrade(request);
    return this.hook(request, url, parts.slice(2));
  }

  // ---- inbound event ---------------------------------------------------------------------------------------------

  private takeToken(): boolean {
    const rate = intVar(this.env.RATE_PER_SECOND, 10);
    const burst = intVar(this.env.RATE_BURST, 50);
    const now = Date.now();
    if (this.tokens === null) {
      this.tokens = burst;
      this.refilled = now;
    }
    this.tokens = Math.min(burst, this.tokens + ((now - this.refilled) / 1000) * rate);
    this.refilled = now;
    if (this.tokens < 1) return false;
    this.tokens -= 1;
    return true;
  }

  private async readCapped(request: Request, max: number): Promise<Uint8Array | null> {
    if (Number(request.headers.get("content-length") ?? "0") > max) return null;
    const chunks: Uint8Array[] = [];
    let total = 0;
    if (request.body) {
      const reader = request.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.length;
        if (total > max) {
          await reader.cancel();
          return null;
        }
        chunks.push(value);
      }
    }
    const out = new Uint8Array(total);
    let o = 0;
    for (const c of chunks) {
      out.set(c, o);
      o += c.length;
    }
    return out;
  }

  private async hook(request: Request, url: URL, suffix: string[]): Promise<Response> {
    if (request.method !== "POST") return new Response(null, { status: 405, headers: { allow: "POST" } });
    if (!this.takeToken()) return new Response(null, { status: 429, headers: { "retry-after": "1" } });
    const body = await this.readCapped(request, intVar(this.env.MAX_BODY_BYTES, 1 << 20));
    if (body === null) return new Response(null, { status: 413 });

    const h = request.headers;
    const deliveryId =
      h.get("x-github-delivery") ?? h.get("x-delivery-id") ?? h.get("idempotency-key") ?? crypto.randomUUID();
    const sql = this.ctx.storage.sql;
    const cur = sql.exec(
      `INSERT INTO events (delivery_id, received_at, method, path_suffix, query, headers, body, size)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(delivery_id) DO NOTHING`,
      deliveryId, Date.now(), request.method, suffix.join("/"), url.search, JSON.stringify(passHeaders(h)), body, body.length,
    );
    cur.toArray();
    if (cur.rowsWritten === 0) return new Response(null, { status: 200 }); // repeat delivery: acknowledged, stored once

    this.enforceCaps();
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + intVar(this.env.TTL_SECONDS, 86400) * 1000);
    }
    const row = sql.exec("SELECT * FROM events WHERE delivery_id = ?", deliveryId).toArray()[0];
    if (row) this.pushLive(row);
    return new Response(null, { status: 202 });
  }

  private enforceCaps() {
    const sql = this.ctx.storage.sql;
    const maxEvents = intVar(this.env.MAX_EVENTS, 1000);
    const maxBytes = intVar(this.env.MAX_BUFFER_BYTES, 64 << 20);
    let dropped = 0;
    for (;;) {
      const t = sql.exec("SELECT COUNT(*) AS n, COALESCE(SUM(size), 0) AS b FROM events").one();
      if ((t.n as number) <= maxEvents && (t.b as number) <= maxBytes) break;
      sql.exec("DELETE FROM events WHERE seq = (SELECT MIN(seq) FROM events)");
      dropped += 1;
    }
    if (dropped) this.setMeta("dropped", String(Number(this.meta("dropped") ?? "0") + dropped));
  }

  // ---- client socket ---------------------------------------------------------------------------------------------

  private async upgrade(request: Request): Promise<Response> {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") return new Response(null, { status: 426 });
    const m = /^Bearer (.+)$/.exec(request.headers.get("authorization") ?? "");
    const stored = this.meta("token_hash");
    if (!m || stored === null) return new Response(null, { status: 401 });
    // Compare digests of both sides so the comparison is constant-time and length-independent.
    const presented = await sha256(b64url(await sha256(m[1])));
    if (!timingSafeEqual(presented, await sha256(stored))) return new Response(null, { status: 401 });

    for (const old of this.ctx.getWebSockets()) {
      this.sendTo(old, { type: "error", code: "replaced", message: "another client connected" });
      old.close(1000, "replaced");
    }
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1]);
    pair[1].serializeAttachment({ hello: false } satisfies Attachment);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private sendTo(ws: WebSocket, frame: unknown) {
    try {
      ws.send(JSON.stringify(frame));
    } catch {
      /* socket already closing */
    }
  }

  private frame(row: Row) {
    return {
      type: "event",
      seq: row.seq,
      delivery_id: row.delivery_id,
      received_at: row.received_at,
      method: row.method,
      path_suffix: row.path_suffix,
      query: row.query,
      headers: JSON.parse(row.headers as string),
      body_b64: b64(new Uint8Array(row.body as ArrayBuffer)),
    };
  }

  private pushLive(row: Row) {
    for (const ws of this.ctx.getWebSockets()) {
      if ((ws.deserializeAttachment() as Attachment | null)?.hello) this.sendTo(ws, this.frame(row));
    }
  }

  webSocketMessage(ws: WebSocket, message: string | ArrayBuffer) {
    if (typeof message !== "string") return ws.close(1003, "text frames only");
    let f: { type?: string; last_acked_seq?: number; seq?: number };
    try {
      f = JSON.parse(message);
    } catch {
      return ws.close(1007, "bad json");
    }
    const sql = this.ctx.storage.sql;
    if (f.type === "hello") {
      const last = Number.isInteger(f.last_acked_seq) ? (f.last_acked_seq as number) : 0;
      sql.exec("DELETE FROM events WHERE seq <= ?", last);
      this.sendTo(ws, {
        type: "welcome",
        v: 1,
        max_event_bytes: intVar(this.env.MAX_BODY_BYTES, 1 << 20),
        ttl_seconds: intVar(this.env.TTL_SECONDS, 86400),
        dropped: Number(this.meta("dropped") ?? "0"),
        server_time: Date.now(),
      });
      for (const row of sql.exec("SELECT * FROM events WHERE seq > ? ORDER BY seq", last)) this.sendTo(ws, this.frame(row));
      ws.serializeAttachment({ hello: true } satisfies Attachment);
    } else if (f.type === "ack" && Number.isInteger(f.seq)) {
      sql.exec("DELETE FROM events WHERE seq <= ?", f.seq as number);
    }
  }

  webSocketClose(ws: WebSocket, code: number) {
    try {
      ws.close(code === 1005 ? 1000 : code);
    } catch {
      /* already closed */
    }
  }

  // ---- expiry ----------------------------------------------------------------------------------------------------

  async alarm() {
    const ttl = intVar(this.env.TTL_SECONDS, 86400) * 1000;
    const sql = this.ctx.storage.sql;
    sql.exec("DELETE FROM events WHERE received_at <= ?", Date.now() - ttl);
    const next = sql.exec("SELECT MIN(received_at) AS t FROM events").one().t as number | null;
    if (next !== null) await this.ctx.storage.setAlarm(next + ttl);
  }
}
