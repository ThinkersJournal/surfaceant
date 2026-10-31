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

## Second spike: the three open items (2026-10-31)

Added to `spike/rust` (`Channel::check`, routes under `/check/`), tested in `spike/rust/test/spike.test.ts` (tests 3-5). Command for all
three: `cd spike/rust && worker-build --release && npx vitest run` -> **5 passed (5)**. New crates: `hmac` 0.13.0, `sha2` 0.11.1,
`subtle` 2.6.1, `futures-util` 0.3.34. After adding them: bundle **507.91 KiB / 173.39 KiB gzip** (was 472.45 / 160.89, so crypto cost
about 35 KiB raw / 12.5 KiB gzip); `cargo audit` 0 vulnerabilities over 96 crates; 122 distinct crate versions in the wasm32 tree.

| Item | Result | Evidence |
|---|---|---|
| HMAC-SHA-256 + constant-time compare in wasm | **PASS** | Pure-Rust `hmac`+`sha2`; `Mac::verify_slice` (constant-time, via `subtle`). Test: RFC 4231 test case 2 (key `Jefe`, data `what do ya want for nothing?`, MAC `5bdcc146...ec3843`) verifies 200; one changed message char, one changed MAC nibble, and non-hex MAC all give 401 (the negative controls). No `getrandom` was needed: verification needs no randomness. **Channel-id generation does** (random bytes); use the Workers `crypto.getRandomValues` through `getrandom` with its wasm-js backend, or generate ids in the admin path through `web-sys` `Crypto`: **not built, UNVERIFIED** |
| DO alarms in Rust | **PASS** | `DurableObject::alarm` + `State::storage().set_alarm(Duration)` (100 ms offset). Test: `alarm-fired` count 0 -> set -> wait 600 ms -> 1. Also confirmed under real `wrangler dev` (handler entered, `rows_written` 1). `get_alarm`/`delete_alarm` exist but were not run |
| Capped streaming body reader | **PASS** | `Request::stream()` (`ByteStream`, a `futures` `Stream`) read chunk by chunk, abort at the cap. Test: 4096 B under a 4096 cap -> 200 and "4096"; a 5 MiB chunked body with no `Content-Length` -> 413 (the reader returns at the cap, the rest is not buffered) |

My first run of the alarm test **failed**; the cause was my probe, not workers-rs: the entry point routed `/check/alarm-set` and
`/check/alarm-fired` to two different Durable Objects (it keys on the second path segment). Established by logging inside the handler
under `wrangler dev` (handler ran, insert wrote 1 row, the read hit another object), then routing all `/check/*` to one object.

Still **UNVERIFIED** (none blocks the choice): production CPU/cold start on Cloudflare; the Free-plan script-size limit against 173 KiB
gzip; secure random in wasm for id generation; frame types compiled into the client crate; a serde-decoded BLOB column.

Tests: **the Rust Worker is still tested through vitest/wrangler (Node)**, so the Node toolchain stays in CI either way; Rust only
removes TypeScript from the production code. Pure logic (ids, framing, caps) can also be unit-tested with host `cargo test` if it
lives in a non-wasm crate.

## Reading the numbers

- TypeScript wins on bundle size (about 250x), build time and API currency (first-class bindings, no wasm layer).
- Locally there is no visible runtime difference; the real difference, if any, is in production CPU/cold start and is unmeasured.
- Rust passed every functional requirement in the spike with no code workaround, at the cost of a slower loop and a larger
  artifact. Nothing measured blocks it.
- Rust's concrete gains: one language, and one shared `surfaceant-proto` crate for the wire frames between Worker and client
  (the part most likely to drift).

## Recommendation

**Rust (workers-rs)**. All three conditions passed (second spike). CireSnave prefers Rust, no functional gap was found, and the
shared protocol crate removes a drift risk between Worker and client. If a later gap appears (the remaining UNVERIFIED items),
fall back to TypeScript for the Worker (the wire protocol and test vectors are
language-neutral, so the client and the design do not change). Costs to accept: roughly 500 KiB (173 KiB gzip) of artifact per deploy, a
15-second release-profile loop (use the dev profile while iterating), and a pre-1.0 SDK (`worker` 0.8.x) whose API may move.
