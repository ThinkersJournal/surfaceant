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
| 4 | Deploy to Cloudflare (`thinkersjournal.net`) | blocked | Script, runbook and smoke test merged in #6 ([`docs/DEPLOY.md`](docs/DEPLOY.md), `deploy/deploy.sh`, `examples/smoke.rs`; dry run and local smoke verified). The deploy itself waits on board 445 (token, go, plan); then production CPU/cold start is recorded. |
| 5 | Abuse / privacy / terms decision items for CireSnave | review | Ruled 2026-11-16 (board 447, [`docs/ROW5-DECISIONS.md`](docs/ROW5-DECISIONS.md)). Built: 5-minute buffer TTL (#11), per-service abuse contact (#12), notice and terms text ([`docs/NOTICE-AND-TERMS.md`](docs/NOTICE-AND-TERMS.md)). Open: the subdomain model ([`docs/DESIGN-SUBDOMAINS.md`](docs/DESIGN-SUBDOMAINS.md), options A/B/C awaiting a ruling). Launch scope stays people CireSnave names. |
| 6 | Surface mode (HTTP proxied to `<name>.thinkersjournal.net`) | later | Separate design. Not started before row 4 is delivered and CireSnave rules. Needs a Public Suffix List entry before any public user subdomain. |
