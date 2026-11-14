import { SELF } from "cloudflare:test";
import { expect, it } from "vitest";

const get = async () => (await SELF.fetch("https://relay.test/check/random")).text();

it("channel-id randomness: 32 hex digits, different every call (getrandom -> crypto.getRandomValues)", async () => {
  const ids = await Promise.all(Array.from({ length: 64 }, get));
  for (const id of ids) expect(id).toMatch(/^[0-9a-f]{32}$/);
  expect(new Set(ids).size).toBe(64); // 64 x 128 bits colliding would mean a constant or weak source
  expect(ids.join("")).not.toMatch(/^(0{32})+$/); // negative control: not an all-zero buffer
});

it("unknown path is 404", async () => {
  expect((await SELF.fetch("https://relay.test/nope")).status).toBe(404);
});
