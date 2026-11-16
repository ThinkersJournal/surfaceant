# Changelog

Versions are allocated by the portfolio PM at merge time; one version covers every crate in the workspace.

## 0.0.16

- docs: DESIGN-SUBDOMAINS.md option (e), one delegated child zone on two VPS nameservers, with the delegation facts (verified/UNVERIFIED); option (d) corrected with the NS and zone-record limits.

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
