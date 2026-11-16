// SPDX-License-Identifier: MIT OR Apache-2.0
import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({ plugins: [cloudflareTest({ wrangler: { configPath: "./wrangler.jsonc" } })] });
