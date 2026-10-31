# Spike: Worker language, TypeScript vs Rust (workers-rs)

Run 2026-10-31 on one Windows 11 box, **local only** (nothing deployed to Cloudflare). Code: `spike/ts` and `spike/rust`
(branch `spike-worker-language`). Both implement the same minimal Worker: a Durable Object per channel that (a) accepts a
hibernating WebSocket (`acceptWebSocket` / `accept_web_socket`), (b) accepts `POST /hooks/<id>` and forwards headers and the raw
body to the socket, (c) stores each event in the DO's SQLite while no socket is connected and replays it on connect, (d) deletes on
`{"ack": seq}`. **Both are exercised by the same test file** (`test/spike.test.ts`: non-UTF-8 body byte-identical, header
preserved, offline buffer replays, ack deletes), run under `@cloudflare/vitest-plugin`; both pass 2/2.

Versions: node 26.5.0, wrangler 4.156.0, vitest 5.0.5, `@cloudflare/vitest-plugin` 1.4.11 (`@cloudflare/vitest-pool-workers` is
**deprecated and renamed** to it; its latest only accepts vitest ^4), typescript 7.0.2, `@cloudflare/workers-types`
5.20261031.1; rustc 1.99.1, `worker` 0.8.7, `worker-build` 0.8.7 (wasm-opt 132, esbuild 0.28.2).

## Results

| Measure | TypeScript | Rust (workers-rs) | Command / source |
|---|---|---|---|
| Bundle, raw / gzip (as uploaded) | **1.88 KiB / 0.94 KiB** | **472.45 KiB / 160.89 KiB** (wasm 441,967 B + shim 41,826 B) | `npx wrangler deploy --dry-run --outdir dist-x` in each dir; "Total Upload" line |
| Local time per POST, first request (cold: isolate, DO, wasm init) | 130.0 ms | 138.5 ms | `scratchpad/lat.mjs`: `wrangler dev`, 1 request |
| Local time per POST, steady (n=300) p50 / p95 | 15.7 / 24.1 ms | 15.8 / 27.2 ms | same script; wall time through the local dev proxy |
| `wrangler dev` start to "Ready" | 3.8 s | 4.2 s | same script |
| Edit-test loop, warm | **4.5-4.8 s** (`vitest run`) | dev profile: 3.6 s build + 5.7-6.8 s test; release: 12.7-13.8 s build + 4.5-4.7 s test | `scratchpad/loop.mjs`, 2 runs each |
| Edit-test loop, cold | 6.5 s first `vitest run` after install (+ `npm i`) | 3 m 20 s one-time `cargo install worker-build`, then 1 m 23 s first release build (deps), 8.6 s first `vitest run` | `time` wrapped; first runs |
| Lines of code (Worker source) | 47 | 78 | `wc -l` (rustfmt-ed Rust is longer; tests are the same 40 lines) |
| Dependencies | 0 prod; 73 packages in the dev tree (`npm ls --all --parseable`) | 109 distinct crate versions in the normal tree for wasm32 (`cargo tree -e normal --prefix none \| sort -u`); `cargo audit` scanned 83 crates | |
| Audit | `npm audit`: 0 vulnerabilities (also `--omit=dev`) | `cargo audit`: 0 vulnerabilities, 1,395 advisories loaded | 2026-10-31 |
| Hibernation API | works (`acceptWebSocket`, `getWebSockets`) | **works** from workers-rs 0.8.7: `State::accept_web_socket`, `get_websockets`, `set_websocket_auto_response`, `WebSocket::serialize_attachment`, and the `websocket_message` / `websocket_close` trait methods | Test passes in both; Rust API read in `worker-0.8.7/src/durable.rs`, `websocket.rs` |
| DO SQLite storage | works (`ctx.storage.sql`) | **works**: `state.storage().sql().exec(query, bindings)`; typed rows via serde, raw rows via `.raw()` yielding `SqlStorageValue::{Integer,String,Blob,..}`; blobs round-trip byte-identically (tested) | same |
| Workarounds needed | none | none in code. **Environment:** one `cargo check` failed with `sccache: encountered fatal error ... failed to spawn Command { ... rustc.exe ... web_sys ...}` (the box's compiler wrapper); `RUSTC_WRAPPER= cargo ...` fixed it. Typed-row decoding of a BLOB column was avoided by using `.raw()` (not tried with serde: UNVERIFIED) | |
| Share frame types with the Rust client crate | Not directly: restate in TS, or generate TS from the Rust types (`ts-rs`/`typeshare`) or from a JSON Schema; shared JSON test vectors either way | **Yes**: a small `surfaceant-proto` crate (serde structs) used by Worker and client. The `Frame` struct above is plain serde and compiles for `wasm32-unknown-unknown`; the client build against the same crate was **not run** (UNVERIFIED) | |

Not Cloudflare production numbers: the latency rows are wall time through `wrangler dev` (miniflare/workerd on this box, with
~15 ms of proxy overhead in every request), so they say "no visible difference locally", not what CPU time is billed or limited
on Cloudflare. **Production cold start and CPU time, and the Free-plan script-size limit against 161 KiB gzip: UNVERIFIED.**

## What the spike did not cover (UNVERIFIED, needed for the real Worker)

- HMAC-SHA-256 and constant-time comparison from Rust in wasm: via `web-sys` `SubtleCrypto`, or the pure-Rust `hmac` + `sha2` +
  `subtle` crates (adds size; `getrandom` needs its `js` backend). Not built.
- Durable Object **alarms** in Rust: the `alarm` trait method exists (`durable.rs`); not run.
- Reading a request body with a hard size cap while streaming: `Request::bytes()` buffers whole; a capped stream reader from Rust
  was not tried.
- Tests: **the Rust Worker is still tested through vitest/wrangler (Node)**, so the Node toolchain stays in CI either way; Rust
  only removes TypeScript from the production code. Pure logic (ids, framing, caps) could also be unit-tested with plain
  `cargo test` on the host if it lives in a non-wasm crate.

## Reading the numbers

- TypeScript wins on bundle size (about 250x), build time and API currency (first-class bindings, no wasm layer).
- Locally there is no visible runtime difference; the real difference, if any, is in production CPU/cold start and is unmeasured.
- Rust passed every functional requirement in the spike with no code workaround, at the cost of a slower loop and a larger
  artifact. Nothing measured blocks it.
- Rust's concrete gains: one language, and one shared `surfaceant-proto` crate for the wire frames between Worker and client
  (the part most likely to drift).

## Recommendation

**Rust (workers-rs)**, conditional on a first-hour check in row 2 of the three UNVERIFIED items above (crypto, alarm, capped body).
CireSnave prefers Rust, the spike found no functional gap, and the shared protocol crate removes a drift risk between Worker and
client. If any of the three fails in practice, fall back to TypeScript for the Worker (the wire protocol and test vectors are
language-neutral, so the client and the design do not change). Costs to accept: roughly 470 KiB of artifact per deploy, a
15-second release-profile loop (use the dev profile while iterating), and a pre-1.0 SDK (`worker` 0.8.x) whose API may move.
