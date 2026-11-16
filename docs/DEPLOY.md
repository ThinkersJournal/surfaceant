# Deploying the relay (ROADMAP row 4)

Script: [`deploy/deploy.sh`](../deploy/deploy.sh). Nothing in it reads, prints or stores a secret value; the PM supplies them
as environment variables under `with-secret`.

## Prerequisites (PM)

1. **Cloudflare API token** scoped to what a deploy needs, nothing broader: Account > Workers Scripts: Edit; Zone
   (`thinkersjournal.net`) > Workers Routes: Edit. Durable Object namespaces are created by the deploy itself (migration `v1`).
   Also `CLOUDFLARE_ACCOUNT_ID` if the token can see more than one account. As of 2026-11-14 no such token exists.
2. **Secrets**, 32+ random bytes each, generated under `with-secret` and never written to a file or chat:
   `CHANNEL_KEY` (signs channel ids; rotating it invalidates every channel id) and `ADMIN_SECRET` (guards `/admin/*`).
3. `thinkersjournal.net` is a zone on that account, proxied (orange cloud) for the apex.
4. Plan: Durable Objects with SQLite storage are on the Workers **Free** plan, but Free caps CPU at 10 ms per request. Row 4
   measures real CPU (below); if it is near the cap, the answer is the Paid plan, a CireSnave decision, not a code change.

## Steps

```
deploy/deploy.sh check      # anyone, no credentials: build + dry run (verified 2026-11-14: 249 KiB gzip, bindings and vars as expected)
DEPLOY_GO=1 deploy/deploy.sh deploy     # PM's go only: uploads the Worker, applies migration v1, binds three routes
deploy/deploy.sh secrets    # CHANNEL_KEY, ADMIN_SECRET from env, via stdin
ADMIN_SECRET=... deploy/deploy.sh smoke # live smoke test
```

Order matters: `deploy` first (a secret needs the Worker to exist), then `secrets`, then `smoke`. Until the secrets are set the
Worker answers 500 on provisioning and 404 on every hook; that is the safe default, not a fault.

Routes: `thinkersjournal.net/hooks/*`, `/connect/*`, `/admin/*`. Every other path on the apex is untouched. `workers_dev` is off,
so there is no second public URL. `/admin/*` is public but needs `ADMIN_SECRET` (constant-time compare); if that is not enough,
drop the third route and provision over `wrangler`'s own tooling instead.

## Secrets: who holds them, and rotation

`ADMIN_SECRET` and `CHANNEL_KEY` are generated per run under `with-secret` and **held by nobody afterwards**: they are not in a
file, in git or in chat; only the Worker's secret store has them. To rotate, generate new values under `with-secret` and run
`deploy/deploy.sh secrets` again (no code change). Rotating `ADMIN_SECRET` only changes who can call `/admin/*` (a smoke run
needs the current one, so it is rotated first). **Rotating `CHANNEL_KEY` invalidates every channel id**: ids are HMAC-signed with
it, so every provisioned channel stops resolving and has to be provisioned again. The deploy token cannot read secret values
or the CPU metrics (below).

## TLS in the client

`surfaceant-client` selects rustls's `ring` crypto provider itself, once, before its first connection (`ensure_tls_provider`).
Without that, rustls 0.23 panics on the first `wss://` connection ("Could not automatically determine the process-level
CryptoProvider"); the first live smoke run (2026-11-16) hit exactly that. A program that installs its own provider first
keeps it. Checked by `crates/surfaceant-client/tests/tls.rs` (a `wss://` connect reaches the TLS handshake and returns an error,
no panic; red before the fix) and by the live smoke test, which connects over WSS.

## Smoke test (`crates/surfaceant-client/examples/smoke.rs`)

Provisions a throwaway channel and checks, over real HTTPS/WSS: forged id 404, admin without secret 401, two offline events
buffered (202) and replayed exactly once to a local program, a repeated delivery id answered 200 and not stored twice, ten live
events delivered, the channel disabled and then refusing events. It prints client-observed latency (includes the network and
curl start-up). Same test passes against `wrangler dev` (verified 2026-11-14: 12 deliveries, median 68 ms).

## Live result (2026-11-16)

Deployed by the PM (board 445 A; Worker version `b9f37248`, three routes, secrets set). The live smoke test **passed in full**
at about 21:30Z against `thinkersjournal.net` at main 0.0.15: channel provisioned, forged id 404, admin without secret 401,
offline events buffered and replayed exactly once, a repeated delivery id not stored twice, 12 distinct deliveries, then the
channel disabled and refusing events. Client-observed latency: POST-to-202 and POST-to-delivered-to-the-local-program were
both **median 347 ms, max 354 ms** (includes curl start-up and TLS from the PM's machine; the local `wrangler dev` figure of
68 ms has no network in it); connecting and replaying 2 buffered events took 607.6 ms. The first run (main 52b7dff) deployed
and stopped after 6 checks at the client's rustls provider panic (fixed in #14); the second run is the full pass. The deploy
printed "Total Upload: 746.83 KiB / gzip: 255.56 KiB" and "Worker Startup Time: 8 ms" (wrangler's figure at upload; it is
not a cold-start or CPU measurement). The Worker secrets were rotated after the run and are held by nobody.
**Limit: CPU p50/p99 and cold start were not measured** (below). Source: the PM's raw run output,
`C:\Projects\.lane-state\surfaceant-smoke-2026-11-16.txt` (both runs; hex strings of 40+ characters redacted by the PM, no
secret printed). It lives outside this repository and is not copied here, so no channel id enters git.

## CPU time and cold start (the one unverified spike item)

The Worker cannot report its own CPU: **CPU is read from the dashboard (Metrics); the deploy token cannot read it.** After the smoke run, read it from the dashboard: Workers & Pages > `surfaceant-worker` >
Metrics (CPU time p50/p99, per request) and, for the Durable Object, the Durable Objects tab. Observability is enabled in
`wrangler.jsonc`, so `wrangler tail surfaceant-worker` also shows per-invocation outcomes. Record the p50/p99 and the first
request after idle (cold start) in `docs/SPIKE-WORKER-LANGUAGE.md`, replacing "UNVERIFIED". Acceptance: p99 CPU under the plan's
cap with margin; a WebSocket connect plus replay of 1000 buffered events is the worst case worth running by hand once.

## Roll back

`cd worker && npx wrangler rollback` (previous version), or remove the three routes in the dashboard to take the relay off the
internet without deleting any channel state. Disabling one channel: `POST /admin/channels/<id>/disable`.
