# SurfaceAnt roadmap

SurfaceAnt lets a program on a user's machine be reached from the internet without opening a port: the machine keeps one
outbound WebSocket to a relay, and the relay forwards what arrives for that user's channel down it.

Statuses: `todo` · `doing` · `review` (PR open) · `done` (merged) · `blocked` (named dependency) · `later` (needs a ruling).

| # | Row | Status | Notes |
|---|-----|--------|-------|
| 0 | `ROADMAP.md`, CI (fmt, clippy, test, doc, deny, portfolio), branch protection requiring them | review | CI is in the seed; PM enables protection after this PR's run. |
| 1 | Design note ([`docs/DESIGN.md`](docs/DESIGN.md)): options, recommendation, no code | review | Language choice for the Worker, frames, auth, ack/replay, buffer, limits, failures, mode 2 reuse. |
| 2 | Worker + Durable Object skeleton with tests (events mode) | todo | Needs PM sign-off on the design's open questions. Tests: signed sample byte-identical; offline buffer replays in order once; duplicate delivery id once; oversize / rate limit refused; wrong token refused. |
| 3 | Rust client crate `surfaceant-client` + end-to-end test (local Worker -> client -> `gh_webhook_receiver.py`) | todo | Signed GitHub-shaped sample plus a tampered-byte negative control. |
| 4 | Deploy to Cloudflare (`thinkersjournal.net`) | blocked | Blocked on rows 2-3 passing locally; the PM asks CireSnave for a scoped token. |
| 5 | Abuse / privacy / terms decision items for CireSnave | todo | Written as board items via the PM; not decided here. |
| 6 | Surface mode (HTTP proxied to `<name>.thinkersjournal.net`) | later | Separate design. Not started before row 4 is delivered and CireSnave rules. Needs a Public Suffix List entry before any public user subdomain. |
