import { DurableObject } from "cloudflare:workers";

interface Env {
  CHANNEL: DurableObjectNamespace<Channel>;
}

export default {
  fetch(request: Request, env: Env): Promise<Response> | Response {
    const id = new URL(request.url).pathname.split("/")[2];
    if (!id) return new Response(null, { status: 404 });
    return env.CHANNEL.get(env.CHANNEL.idFromName(id)).fetch(request);
  },
};

export class Channel extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS ev (seq INTEGER PRIMARY KEY AUTOINCREMENT, headers TEXT NOT NULL, body BLOB NOT NULL)",
    );
  }

  private frame(r: Record<string, SqlStorageValue>): string {
    const bytes = new Uint8Array(r.body as ArrayBuffer);
    return JSON.stringify({ seq: r.seq, headers: JSON.parse(r.headers as string), body_b64: btoa(String.fromCharCode(...bytes)) });
  }

  async fetch(request: Request): Promise<Response> {
    const sql = this.ctx.storage.sql;
    if (request.headers.get("upgrade") === "websocket") {
      const [client, server] = Object.values(new WebSocketPair());
      this.ctx.acceptWebSocket(server); // hibernation API
      for (const r of sql.exec("SELECT * FROM ev ORDER BY seq")) server.send(this.frame(r)); // replay
      return new Response(null, { status: 101, webSocket: client });
    }
    const body = new Uint8Array(await request.arrayBuffer());
    const headers = JSON.stringify([...request.headers]);
    const row = sql.exec("INSERT INTO ev (headers, body) VALUES (?, ?) RETURNING *", headers, body).one();
    for (const ws of this.ctx.getWebSockets()) ws.send(this.frame(row));
    return new Response(null, { status: 202 });
  }

  webSocketMessage(_ws: WebSocket, message: string | ArrayBuffer) {
    const m = JSON.parse(message as string);
    if (typeof m.ack === "number") this.ctx.storage.sql.exec("DELETE FROM ev WHERE seq <= ?", m.ack);
  }
}
