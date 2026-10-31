import { SELF } from "cloudflare:test";
import { expect, it } from "vitest";

const post = (id: string, body: Uint8Array, extra: Record<string, string> = {}) =>
  SELF.fetch(`https://x.test/hooks/${id}`, { method: "POST", body, headers: { "x-hub-signature-256": "sha256=abc", ...extra } });

async function connect(id: string) {
  const res = await SELF.fetch(`https://x.test/connect/${id}`, { headers: { upgrade: "websocket" } });
  const ws = res.webSocket!;
  ws.accept();
  const got: any[] = [];
  ws.addEventListener("message", (e) => got.push(JSON.parse(e.data as string)));
  return { ws, got };
}
const settle = () => new Promise((r) => setTimeout(r, 50));
const bytes = new Uint8Array([0, 255, 128, 10, 13, 123]); // non-UTF-8 on purpose

it("forwards raw body and headers to a connected socket (a, b)", async () => {
  const { ws, got } = await connect("live");
  expect((await post("live", bytes)).status).toBe(202);
  await settle();
  expect(got).toHaveLength(1);
  expect(Uint8Array.from(atob(got[0].body_b64), (c) => c.charCodeAt(0))).toEqual(bytes);
  expect(got[0].headers).toContainEqual(["x-hub-signature-256", "sha256=abc"]);
  ws.close();
});

it("buffers while offline, replays on connect, ack deletes (c, d)", async () => {
  await post("off", bytes);
  const a = await connect("off");
  await settle();
  expect(a.got).toHaveLength(1);
  a.ws.send(JSON.stringify({ ack: a.got[0].seq }));
  await settle();
  a.ws.close();
  const b = await connect("off");
  await settle();
  expect(b.got).toHaveLength(0); // deleted by the ack
  b.ws.close();
});

// ---- second spike: HMAC, alarm, capped body ----
const get = (p: string, init?: RequestInit) => SELF.fetch(`https://x.test/check/${p}`, init);

it("HMAC-SHA-256 verifies RFC 4231 case 2 and rejects a tampered mac / message", async () => {
  const mac = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
  const q = (msg: string, m: string) => `hmac?key=Jefe&msg=${encodeURIComponent(msg)}&mac=${m}`;
  expect((await get(q("what do ya want for nothing?", mac), { method: "POST" })).status).toBe(200);
  expect((await get(q("what do ya want for nothing!", mac), { method: "POST" })).status).toBe(401);
  expect((await get(q("what do ya want for nothing?", mac.replace(/.$/, "4")), { method: "POST" })).status).toBe(401);
  expect((await get(q("what do ya want for nothing?", "zz"), { method: "POST" })).status).toBe(401);
});

it("a DO alarm fires", async () => {
  expect(await (await get("alarm-fired")).text()).toBe("0");
  expect((await get("alarm-set", { method: "POST" })).status).toBe(200);
  await new Promise((r) => setTimeout(r, 600));
  expect(await (await get("alarm-fired")).text()).toBe("1");
});

it("capped reader accepts under the cap and refuses over it, streamed with no content-length", async () => {
  const stream = (n: number) =>
    new ReadableStream<Uint8Array>({ pull(c) { c.enqueue(new Uint8Array(1024)); if ((n -= 1024) <= 0) c.close(); } });
  const post = (n: number) => get("capped?max=4096", { method: "POST", body: stream(n), duplex: "half" } as RequestInit);
  const ok = await post(4096);
  expect([ok.status, await ok.text()]).toEqual([200, "4096"]);
  expect((await post(5 * 1024 * 1024)).status).toBe(413);
});
