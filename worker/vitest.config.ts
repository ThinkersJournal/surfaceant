// SPDX-License-Identifier: MIT OR Apache-2.0
import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

// Test-only secrets and small limits; real secrets are Worker secrets and never in git.
export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      miniflare: {
        bindings: {
          CHANNEL_KEY: "test-channel-key",
          ADMIN_SECRET: "test-admin-secret",
          MAX_BODY_BYTES: "4096",
          RATE_PER_SEC: "0.001",
          RATE_BURST: "20",
          WINDOW: "2",
          BUFFER_MAX_EVENTS: "5",
        },
      },
    }),
  ],
});
