# surfaceant

SurfaceAnt lets a program on your machine be reached from the internet without opening a port: the machine keeps one outbound
WebSocket to a relay, and the relay forwards what arrives for your channel down it.

**Status: deployed, live smoke passed (2026-11-16); CPU time and cold start not yet measured.** The relay (a Rust Cloudflare Worker with a Durable Object per channel), the Rust client crate
and the wire protocol exist and are tested in CI (`cargo test`, the Worker's vitest suite, and an end-to-end job that drives
relay -> client -> receiver). The relay runs on `thinkersjournal.net` (see `docs/DEPLOY.md`); production CPU p50/p99 and cold start
are UNVERIFIED (dashboard only). The crates are `publish = false`; the version in `Cargo.toml` and `CHANGELOG.md` is the current one.

- Design: [`docs/DESIGN.md`](docs/DESIGN.md). Roadmap: [`ROADMAP.md`](ROADMAP.md). Deploy runbook: [`docs/DEPLOY.md`](docs/DEPLOY.md).
- Events mode today: a sender POSTs to `/hooks/<channel-id>`; an offline service's events are kept for at most 5 minutes.
- Abuse, privacy and terms text (draft): [`docs/NOTICE-AND-TERMS.md`](docs/NOTICE-AND-TERMS.md). Launch scope is people the
  owner names; user-controlled subdomains are a design question, [`docs/DESIGN-SUBDOMAINS.md`](docs/DESIGN-SUBDOMAINS.md).

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence (`LICENSE-MIT`), at your
option.
