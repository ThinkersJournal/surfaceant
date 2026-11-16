# Changelog

Versions are allocated by the portfolio PM at merge time; one version covers every crate in the workspace.

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
