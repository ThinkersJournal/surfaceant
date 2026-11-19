# Changelog

Versions are allocated by the portfolio PM at merge time; one version covers every crate in the workspace.

## 0.0.20

- edge: slice 2 (part), per-query answer synthesis instead of a zone file (`DESIGN-VPS-EDGE.md` sections 1.1 and 9). `synth.rs` is a pure decision function over registry data (registered users at any depth, reserved names, suspended and unknown names NXDOMAIN, `_acme-challenge` TXT from a live table, minimal `ANY`, `REFUSED` out of zone); `handler.rs` is a hickory `RequestHandler` that answers from it (AA, SOA in authority, EDNS payload advertised at most 1232); `bind_both` serves any handler on UDP and TCP. Tested with 8 unit tests and a real-socket test over the made-up FIXTURE zone `fixture.test`; this removes the wildcard-at-any-depth difference from the file store for synthesised names. Not yet: the SQLite `registry.db`, RRL and TCP caps, a test of a reply over 1232 bytes, snapshot to box B, the usage-log table, `main.rs` wiring; the real-export comparison and Cloudflare diff still wait for the export. CHANGELOG 0.0.19: the breaking client `ws://` change is now the first line (PM request on #18).

## 0.0.19

- **BREAKING (pre-0.1, client): `ws://` is accepted only when the relay host resolves to loopback.** Any other `ws://` relay now stops the client with an error naming the rule; use `wss://` (certificate validated, unchanged). The host is resolved once, every answer must be loopback (`127.0.0.0/8`, `::1`), and the client connects to that checked address. `Config::relay` doc updated, `check_ws_addrs` exported, tests in `crates/surfaceant-client/tests/relay_scheme.rs` (CireSnave 2026-11-17, secure protocols only; PM ruling LOOPBACK-ONLY).
- edge: new crate `surfaceant-edge`, slice 1 of `DESIGN-VPS-EDGE.md` section 9: `hickory-server` 0.26.3 serving one zone from a standard master file on UDP and TCP (`--origin`, `--zone-file`, `--bind`). Tested over real sockets against made-up FIXTURE zones (`fixture.test`, `nowild.test`; not the real export): apex SOA/NS/A/CAA, a named record beating the wildcard, NODATA, NXDOMAIN, TCP, truncation to TCP, `REFUSED` out of zone. **Finding:** hickory's file store matches the apex wildcard at any depth (not RFC 4592), pinned by a test; the real-export comparison and the diff against Cloudflare wait for the export. Not yet: RRL, EDNS 1232 cap, `ANY` handling (CI is workspace-wide, so the crate is already covered). docs: `DESIGN-VPS-EDGE.md` section 10 "Direct mode" (PM task): user `AAAA`/health-checked `A` with the relay as fallback, what clients do (curl and Python measured, others UNVERIFIED), user-side prerequisites, security rules for a user-controlled address, privacy opt-in recommendation, build order; fixture gains a two-`A` name and a test. docs: section 3.6 secure-protocols-only rule (CireSnave 2026-11-17): no plain-HTTP listener, port 80 closed and removed from the firewall list, no redirect, DNS-01 only; `NOTICE-AND-TERMS.md` term 7.

## 0.0.18

- docs: `DESIGN-VPS-EDGE.md` interim-until-PSL (PM task, CireSnave 2026-11-17): section 8.1 interim isolation (events/named users only, `Set-Cookie` `Domain` strip rule and test, passthrough waits, surface-mode gate kept), section 3.5 certificates before the entry (rate-limit request, Google Trust Services fallback read from its own page, ZeroSSL/SSL.com unverified), section 8.2 day-one usage log.

## 0.0.17

- docs: `docs/DESIGN-VPS-EDGE.md` (board 457, option F): `thinkersjournal.net` on two VPS nameservers; authoritative DNS (hickory-server), the relay re-hosted as a server on the same wire protocol, our own wildcard and per-user wildcard TLS with SNI passthrough, user page, backups and restore, cutover and rollback, Public Suffix List, build order. Supersedes option (e) of `DESIGN-SUBDOMAINS.md`.

## 0.0.16

- docs: DESIGN-SUBDOMAINS.md option (e), one delegated child zone on two VPS nameservers, with the delegation facts (verified/UNVERIFIED); option (d) corrected with the NS and zone-record limits. Live smoke result recorded in DEPLOY.md, ROADMAP row 4 (done) and README; CPU/cold start still not measured.

## 0.0.15

- client: select the rustls `ring` crypto provider explicitly, so a `wss://` connection no longer panics (found by the live smoke run); test added. DEPLOY.md: secret custody and rotation, TLS note, CPU is read from the dashboard. docs: DESIGN-SUBDOMAINS.md gains "Nested names and where TLS ends" (facts marked verified/UNVERIFIED, options a-d, recommendation).

## 0.0.14

- docs: board 447 rulings recorded (`docs/ROW5-DECISIONS.md`), draft notice and terms (`docs/NOTICE-AND-TERMS.md`), subdomain design note with options A/B/C (`docs/DESIGN-SUBDOMAINS.md`); ROADMAP row 5 -> review.

## 0.0.13

- relay: optional per-service abuse contact on channel provisioning (`POST /admin/channels` body `{"abuse_contact": email-or-https-url}`), read back with admin-only `GET /admin/channels/<id>`; malformed is 400 (board 447).

## 0.0.12

- relay: buffer TTL 24 h -> 5 minutes (`BUFFER_TTL_SECONDS` = 300; board 447). **Behaviour change:** an event for an offline service is dropped after 5 minutes; an online service still receives it at once.
- README rewritten: no longer a placeholder; states "built, not deployed, UNVERIFIED live".

## 0.0.11

- client: `Config` no longer prints its connection token through `Debug` (CodeRipper SEC-012); test added.
- SPDX headers on every source file (SUP-016); `Channel` documented (MOD-001).

## 0.0.10

- CI: move the shared portfolio workflow to CodeRipper 0.47.5 (`c72a6e0`). The rule-id migration found nothing to rename.
- docs: `docs/ROW5-DECISIONS.md` records the board numbers (447 for items 1-5, 445 for item 6).

## 0.0.9

- docs: ROADMAP row statuses refreshed.

## 0.0.8

- docs: abuse, privacy and terms decision items (`docs/ROW5-DECISIONS.md`).

## 0.0.7

- Deploy script, runbook and live smoke test (`deploy/deploy.sh`, `docs/DEPLOY.md`, `examples/smoke.rs`); `workers_dev` off, observability on.

## 0.0.6

- `surfaceant-client` (connect, replay, ack, reconnect) and the end-to-end example; CI job `e2e`.

## 0.0.5 and earlier

- Rust Worker: channel Durable Object, HMAC channel ids, caps, admin provisioning; `surfaceant-proto`; design note; language spike.
