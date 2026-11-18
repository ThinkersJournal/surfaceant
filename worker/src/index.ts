import { channelIdValid, makeChannelId } from "./channelid";
import { b64url, sha256, timingSafeEqual } from "./util";

export { Channel } from "./channel";

const empty = (status: number, headers?: HeadersInit) => new Response(null, { status, headers });

async function adminOk(request: Request, env: Env): Promise<boolean> {
  const m = /^Bearer (.+)$/.exec(request.headers.get("authorization") ?? "");
  if (!m || !env.ADMIN_TOKEN) return false;
  return timingSafeEqual(await sha256(m[1]), await sha256(env.ADMIN_TOKEN));
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);

    // Admin: create a channel / disable one. The token is returned ONCE; only its hash is stored.
    if (parts[0] === "admin" && parts[1] === "channels") {
      if (!(await adminOk(request, env))) return empty(401);
      if (parts.length === 2 && request.method === "POST") {
        const id = await makeChannelId(env.CHANNEL_HMAC_KEY);
        const token = b64url(crypto.getRandomValues(new Uint8Array(32)));
        const stub = env.CHANNEL.get(env.CHANNEL.idFromName(id));
        await stub.fetch("https://do/init", { method: "POST", body: b64url(await sha256(token)) });
        return Response.json({ channel_id: id, token }, { status: 201, headers: { "cache-control": "no-store" } });
      }
      if (parts.length === 4 && parts[3] === "disable" && request.method === "POST") {
        if (!(await channelIdValid(env.CHANNEL_HMAC_KEY, parts[2]))) return empty(404);
        const stub = env.CHANNEL.get(env.CHANNEL.idFromName(parts[2]));
        return stub.fetch("https://do/disable", { method: "POST" });
      }
      return empty(404);
    }

    // Public: /hooks/<id>[/suffix] receives events; /connect/<id> is the client's WebSocket.
    if ((parts[0] === "hooks" || parts[0] === "connect") && parts[1]) {
      // Unknown or forged ids are answered here, without touching a Durable Object.
      if (!(await channelIdValid(env.CHANNEL_HMAC_KEY, parts[1]))) return empty(404);
      const stub = env.CHANNEL.get(env.CHANNEL.idFromName(parts[1]));
      return stub.fetch(request);
    }
    return empty(404);
  },
} satisfies ExportedHandler<Env>;
