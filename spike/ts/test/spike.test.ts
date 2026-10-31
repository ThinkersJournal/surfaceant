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
