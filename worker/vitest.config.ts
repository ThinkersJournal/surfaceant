import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      miniflare: {
        // Test-only values; real secrets are Worker secrets and never in git.
        bindings: {
          CHANNEL_HMAC_KEY: "test-only-hmac-key-not-a-secret",
          ADMIN_TOKEN: "test-only-admin-token-not-a-secret",
          MAX_EVENTS: "5",
          RATE_PER_SECOND: "1",
          RATE_BURST: "3",
        },
      },
    }),
  ],
});
