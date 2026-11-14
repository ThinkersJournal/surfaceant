# SurfaceAnt roadmap

SurfaceAnt lets a program on a user's machine be reached from the internet without opening a port: the machine keeps one
outbound WebSocket to a relay, and the relay forwards what arrives for that user's channel down it.

Statuses: `todo` · `doing` · `review` (PR open) · `done` (merged) · `blocked` (named dependency) · `later` (needs a ruling).

| # | Row | Status | Notes |
|---|-----|--------|-------|
| 0 | `ROADMAP.md`, CI (fmt, clippy, test, doc, deny, portfolio), branch protection requiring them | review | CI is in the seed; PM enables protection after this PR's run. |
| 1 | Design note ([`docs/DESIGN.md`](docs/DESIGN.md)): options, recommendation, no code | review | Language choice for the Worker, frames, auth, ack/replay, buffer, limits, failures, mode 2 reuse. |
| 2 | Worker + Durable Object skeleton with tests (events mode) | done | Rust (workers-rs), CireSnave 2026-11-14 (board 438). Merged in #3 and #4: channel DO, HMAC ids, caps, admin provisioning, 15 vitest + 8 host tests. Production CPU/cold start is measured at row 4. |
| 3 | Rust client crate `surfaceant-client` + end-to-end test (local Worker -> client -> `gh_webhook_receiver.py`) | review | `cargo run -p surfaceant-client --example e2e` (CI job `e2e`): signed GitHub-shaped sample, offline replay, duplicate, tampered-byte negative control (401), disable. Run against both an in-process receiver and `tools/gh_webhook_receiver.py` on 2026-11-14. |
| 4 | Deploy to Cloudflare (`thinkersjournal.net`) | blocked | Blocked on rows 2-3 passing locally; the PM asks CireSnave for a scoped token. |
| 5 | Abuse / privacy / terms decision items for CireSnave | todo | Written as board items via the PM; not decided here. |
| 6 | Surface mode (HTTP proxied to `<name>.thinkersjournal.net`) | later | Separate design. Not started before row 4 is delivered and CireSnave rules. Needs a Public Suffix List entry before any public user subdomain. |
