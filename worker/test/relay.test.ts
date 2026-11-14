import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

const BASE = "https://relay.test";
const ADMIN = { Authorization: "Bearer test-admin-secret" };

async function provision() {
  const r = await SELF.fetch(`${BASE}/admin/channels`, { method: "POST", headers: ADMIN });
  expect(r.status).toBe(201);
  return (await r.json()) as { channel_id: string; token: string };
}

async function post(id: string, body: BodyInit, headers: Record<string, string> = {}) {
  return SELF.fetch(`${BASE}/hooks/${id}`, { method: "POST", headers, body });
}

type Msg = Record<string, any>;

async function connect(id: string, token: string) {
  const r = await SELF.fetch(`${BASE}/connect/${id}`, {
    headers: { Upgrade: "websocket", Authorization: `Bearer ${token}` },
  });
  expect(r.status).toBe(101);
  const ws = r.webSocket!;
  ws.accept();
  const inbox: Msg[] = [];
  const waiters: Array<() => void> = [];
  ws.addEventListener("message", (e) => {
    inbox.push(JSON.parse(e.data as string));
    waiters.splice(0).forEach((w) => w());
  });
  const next = async (): Promise<Msg> => {
    for (let i = 0; i < 50 && inbox.length === 0; i++) {
      await new Promise<void>((res) => {
        waiters.push(res);
        setTimeout(res, 100);
      });
    }
    if (inbox.length === 0) throw new Error("no frame within 5 s");
    return inbox.shift()!;
  };
  const hello = async (last = 0) => {
    ws.send(JSON.stringify({ type: "hello", v: 1, last_acked_seq: last }));
    return next();
  };
  return { ws, next, hello, inbox, ack: (seq: number) => ws.send(JSON.stringify({ type: "ack", seq })) };
}

const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

describe("provisioning and ids", () => {
  it("admin endpoints refuse a missing or wrong secret", async () => {
    expect((await SELF.fetch(`${BASE}/admin/channels`, { method: "POST" })).status).toBe(401);
    const bad = { Authorization: "Bearer nope" };
    expect((await SELF.fetch(`${BASE}/admin/channels`, { method: "POST", headers: bad })).status).toBe(401);
  });

  it("unknown, forged and malformed ids are 404 with an empty body", async () => {
    const { channel_id } = await provision();
    const forged = channel_id.slice(0, 63) + (channel_id.endsWith("0") ? "1" : "0");
    for (const id of [forged, "0".repeat(64), "short", "x".repeat(64)]) {
      const r = await post(id, "x");
      expect(r.status).toBe(404);
      expect(await r.text()).toBe("");
    }
    expect((await post(channel_id, "x")).status).toBe(202); // positive control: the real id works
  });

  it("only POST on /hooks", async () => {
    const { channel_id } = await provision();
    expect((await SELF.fetch(`${BASE}/hooks/${channel_id}`)).status).toBe(405);
  });
});

describe("delivery", () => {
  it("a signed non-UTF-8 sample arrives byte-identical with its headers and the sender's delivery id", async () => {
    const { channel_id, token } = await provision();
    const body = new Uint8Array([0xff, 0xfe, 0x00, 0x80, 0x0a]);
    const sig = "sha256=" + "ab".repeat(32);
    const r = await post(channel_id, body, {
      "X-Hub-Signature-256": sig,
      "X-GitHub-Delivery": "gh-1",
      "X-GitHub-Event": "push",
      "Content-Type": "application/octet-stream",
    });
    expect(r.status).toBe(202);
    const c = await connect(channel_id, token);
    expect((await c.hello()).type).toBe("welcome");
    const ev = await c.next();
    expect(ev.type).toBe("event");
    expect(ev.delivery_id).toBe("gh-1");
    expect(ev.method).toBe("POST");
    expect(ev.body_b64).toBe(b64(body));
    const h = Object.fromEntries(ev.headers.map(([k, v]: [string, string]) => [k.toLowerCase(), v]));
    expect(h["x-hub-signature-256"]).toBe(sig);
    expect(h["x-github-event"]).toBe("push");
    expect(h["host"]).toBeUndefined(); // hop-by-hop dropped
  });

  it("an offline buffer replays in order, once, and not again after ack", async () => {
    const { channel_id, token } = await provision();
    for (const n of ["a", "b"]) expect((await post(channel_id, n, { "X-GitHub-Delivery": n })).status).toBe(202);
    const c = await connect(channel_id, token);
    await c.hello();
    const first = await c.next();
    const second = await c.next();
    expect([first.delivery_id, second.delivery_id]).toEqual(["a", "b"]);
    expect(second.seq).toBeGreaterThan(first.seq);
    c.ack(second.seq);
    await new Promise((r) => setTimeout(r, 300));
    const c2 = await connect(channel_id, token); // replaces c
    const w = await c2.hello(0);
    expect(w.type).toBe("welcome");
    await new Promise((r) => setTimeout(r, 300));
    expect(c2.inbox).toEqual([]); // acked events are gone: nothing replays
  });

  it("a live event reaches a connected client, and the window holds back the rest until ack", async () => {
    const { channel_id, token } = await provision(); // WINDOW=2 in tests
    const c = await connect(channel_id, token);
    await c.hello();
    for (const n of ["1", "2", "3"]) await post(channel_id, n, { "X-GitHub-Delivery": n });
    const e1 = await c.next();
    const e2 = await c.next();
    await new Promise((r) => setTimeout(r, 300));
    expect(c.inbox).toEqual([]); // third is held: 2 unacked
    c.ack(e2.seq);
    const e3 = await c.next();
    expect([e1.delivery_id, e2.delivery_id, e3.delivery_id]).toEqual(["1", "2", "3"]);
  });

  it("a duplicate delivery id is answered 200 and delivered once", async () => {
    const { channel_id, token } = await provision();
    expect((await post(channel_id, "x", { "X-GitHub-Delivery": "dup" })).status).toBe(202);
    expect((await post(channel_id, "x", { "X-GitHub-Delivery": "dup" })).status).toBe(200);
    const c = await connect(channel_id, token);
    await c.hello();
    expect((await c.next()).delivery_id).toBe("dup");
    await new Promise((r) => setTimeout(r, 300));
    expect(c.inbox).toEqual([]);
  });

  it("past BUFFER_MAX_EVENTS the oldest are dropped and the next welcome says how many", async () => {
    const { channel_id, token } = await provision(); // cap 5
    for (let i = 0; i < 7; i++) await post(channel_id, "e", { "X-GitHub-Delivery": `d${i}` });
    const c = await connect(channel_id, token);
    const w = await c.hello();
    expect(w.dropped_events).toBe(2);
    expect((await c.next()).delivery_id).toBe("d2");
  });
});

describe("refusals", () => {
  it("an oversize body is 413 by Content-Length and by streaming, a body at the cap is accepted", async () => {
    const { channel_id } = await provision(); // MAX_BODY_BYTES=4096
    expect((await post(channel_id, new Uint8Array(4096))).status).toBe(202); // control: at the cap
    expect((await post(channel_id, new Uint8Array(4097))).status).toBe(413);
    const chunked = new ReadableStream({
      start(c) {
        for (let i = 0; i < 6; i++) c.enqueue(new Uint8Array(1024));
        c.close();
      },
    });
    const r = await SELF.fetch(`${BASE}/hooks/${channel_id}`, { method: "POST", body: chunked, duplex: "half" } as RequestInit);
    expect(r.status).toBe(413);
  });

  it("a wrong, missing or malformed token is 401 on connect and a good one is 101", async () => {
    const { channel_id, token } = await provision();
    for (const auth of [undefined, "Bearer wrong", "Basic abc", `Bearer ${token.slice(1)}`]) {
      const h: Record<string, string> = { Upgrade: "websocket" };
      if (auth) h.Authorization = auth;
      expect((await SELF.fetch(`${BASE}/connect/${channel_id}`, { headers: h })).status).toBe(401);
    }
    const ok = await SELF.fetch(`${BASE}/connect/${channel_id}`, { headers: { Upgrade: "websocket", Authorization: `Bearer ${token}` } });
    expect(ok.status).toBe(101);
    ok.webSocket!.accept();
  });

  it("a second connection replaces the first, which is told 'replaced'", async () => {
    const { channel_id, token } = await provision();
    const a = await connect(channel_id, token);
    await a.hello();
    const b = await connect(channel_id, token);
    const err = await a.next();
    expect(err).toMatchObject({ type: "error", code: "replaced" });
    expect((await b.hello()).type).toBe("welcome");
  });

  it("a disabled channel answers 410 and drops the socket", async () => {
    const { channel_id, token } = await provision();
    const c = await connect(channel_id, token);
    await c.hello();
    const d = await SELF.fetch(`${BASE}/admin/channels/${channel_id}/disable`, { method: "POST", headers: ADMIN });
    expect(d.status).toBe(204);
    expect(await c.next()).toMatchObject({ type: "error", code: "disabled" });
    expect((await post(channel_id, "x")).status).toBe(410);
    expect((await SELF.fetch(`${BASE}/connect/${channel_id}`, { headers: { Upgrade: "websocket", Authorization: `Bearer ${token}` } })).status).toBe(410);
    expect((await SELF.fetch(`${BASE}/admin/channels/${channel_id}/disable`, { method: "POST" })).status).toBe(401);
  });

  it("an unsupported protocol version and a non-hello first frame are refused", async () => {
    const { channel_id, token } = await provision();
    const c = await connect(channel_id, token);
    c.ws.send(JSON.stringify({ type: "hello", v: 2, last_acked_seq: 0 }));
    expect(await c.next()).toMatchObject({ type: "error", code: "unsupported_version" });
    const d = await connect(channel_id, token);
    d.ws.send(JSON.stringify({ type: "ack", seq: 1 }));
    expect(await d.next()).toMatchObject({ type: "error", code: "protocol" });
  });
});

describe("rate limit", () => {
  it("a per-channel token bucket (burst 20 in tests) answers 429 with Retry-After, and another channel is unaffected", async () => {
    const { channel_id } = await provision();
    const other = await provision();
    const codes: number[] = [];
    let retry: string | null = null;
    for (let i = 0; i < 25; i++) {
      const r = await post(channel_id, "x", { "X-GitHub-Delivery": `r${i}` });
      codes.push(r.status);
      if (r.status === 429) retry = r.headers.get("Retry-After");
    }
    expect(codes.slice(0, 20).every((c) => c === 202)).toBe(true); // control: the burst is allowed
    expect(codes.slice(20).every((c) => c === 429)).toBe(true);
    expect(Number(retry)).toBeGreaterThanOrEqual(1);
    expect((await post(other.channel_id, "x")).status).toBe(202);
  });
});

describe("randomness", () => {
  it("provisioned channel ids and tokens are 64 hex digits and never repeat", async () => {
    const all = await Promise.all(Array.from({ length: 16 }, provision));
    for (const { channel_id, token } of all) {
      expect(channel_id).toMatch(/^[0-9a-f]{64}$/);
      expect(token).toMatch(/^[0-9a-f]{64}$/);
    }
    expect(new Set(all.map((a) => a.channel_id)).size).toBe(16);
    expect(new Set(all.map((a) => a.token)).size).toBe(16);
  });
});
