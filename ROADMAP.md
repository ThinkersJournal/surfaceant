# SurfaceAnt roadmap

SurfaceAnt lets a program on a user's machine be reached from the internet without opening a port: the machine keeps one
outbound WebSocket to a relay, and the relay forwards what arrives for that user's channel down it.

Statuses: `todo` · `doing` · `review` (PR open) · `done` (merged) · `blocked` (named dependency) · `later` (needs a ruling).

| # | Row | Status | Notes |
|---|-----|--------|-------|
| 0 | `ROADMAP.md`, CI (fmt, clippy, test, doc, deny, portfolio), branch protection requiring them | done | Merged; `main` requires the checks, including `worker (wasm build + vitest)` and `e2e (relay + client + receiver)`. |
| 1 | Design note ([`docs/DESIGN.md`](docs/DESIGN.md)): options, recommendation, no code | done | Merged in #1 and amended with the build; language ruled Rust (board 438). |
| 2 | Worker + Durable Object skeleton with tests (events mode) | done | Rust (workers-rs), CireSnave 2026-11-14 (board 438). Merged in #3 and #4: channel DO, HMAC ids, caps, admin provisioning, 15 vitest + 8 host tests. Production CPU/cold start is measured at row 4. |
| 3 | Rust client crate `surfaceant-client` + end-to-end test (local Worker -> client -> `gh_webhook_receiver.py`) | done | Merged in #5. `cargo run -p surfaceant-client --example e2e` (CI job `e2e`): signed GitHub-shaped sample, offline replay, duplicate, tampered-byte negative control (401), disable. Run against both an in-process receiver and `tools/gh_webhook_receiver.py` on 2026-11-14. |
| 4 | Deploy to Cloudflare (`thinkersjournal.net`) | done | Deployed by the PM on board 445 A (Worker version `b9f37248`, three routes, secrets set). Live smoke PASSED in full on 2026-11-16 ~21:30Z against `thinkersjournal.net` at main 0.0.15 (the first run found the rustls provider panic, fixed in #14): see [`docs/DEPLOY.md`](docs/DEPLOY.md). **Not measured: production CPU p50/p99 and cold start** (dashboard Metrics only; the deploy token cannot read them). |
| 5 | Abuse / privacy / terms decision items for CireSnave | review | Ruled 2026-11-16 (board 447, [`docs/ROW5-DECISIONS.md`](docs/ROW5-DECISIONS.md)). Built: 5-minute buffer TTL (#11), per-service abuse contact (#12), notice and terms text ([`docs/NOTICE-AND-TERMS.md`](docs/NOTICE-AND-TERMS.md)). Open: the subdomain model ([`docs/DESIGN-SUBDOMAINS.md`](docs/DESIGN-SUBDOMAINS.md), options A/B/C awaiting a ruling). Launch scope stays people CireSnave names. |
| 4b | VPS edge: `thinkersjournal.net` on two VPS nameservers (board 457 F); relay re-hosted as a server | review | Design: [`docs/DESIGN-VPS-EDGE.md`](docs/DESIGN-VPS-EDGE.md); slice 1 (DNS server serving a copy of the zone, tested with `dig`) is next. The Cloudflare Worker is temporary and moves under `.com`. |
| 6 | Surface mode (HTTP proxied to `<name>.thinkersjournal.net`) | later | Separate design. Not started before row 4 is delivered and CireSnave rules. Needs a Public Suffix List entry before any public user subdomain. |
