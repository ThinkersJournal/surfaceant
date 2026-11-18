import { b64url, fromB64url, timingSafeEqual } from "./util";

const enc = new TextEncoder();
const RAND = 16;
const MAC = 16;

async function mac(key: string, rand: Uint8Array): Promise<Uint8Array> {
  const k = await crypto.subtle.importKey("raw", enc.encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const full = new Uint8Array(await crypto.subtle.sign("HMAC", k, rand as BufferSource));
  return full.subarray(0, MAC);
}

/** id = base64url(rand[16] || HMAC-SHA256(key, rand)[0..16]). Self-validating: forged ids are rejected without a lookup. */
export async function makeChannelId(key: string): Promise<string> {
  const rand = crypto.getRandomValues(new Uint8Array(RAND));
  const out = new Uint8Array(RAND + MAC);
  out.set(rand, 0);
  out.set(await mac(key, rand), RAND);
  return b64url(out);
}

export async function channelIdValid(key: string, id: string): Promise<boolean> {
  const raw = fromB64url(id);
  if (!raw || raw.length !== RAND + MAC) return false;
  return timingSafeEqual(raw.subarray(RAND), await mac(key, raw.subarray(0, RAND)));
}
