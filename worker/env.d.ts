declare namespace Cloudflare {
  interface Env {
    CHANNEL: DurableObjectNamespace<import("./src/channel").Channel>;
    CHANNEL_HMAC_KEY: string;
    ADMIN_TOKEN: string;
    MAX_BODY_BYTES: string;
    TTL_SECONDS: string;
    MAX_EVENTS: string;
    MAX_BUFFER_BYTES: string;
    RATE_PER_SECOND: string;
    RATE_BURST: string;
  }
}
interface Env extends Cloudflare.Env {}
