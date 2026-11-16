# Design note: user-controlled services on `*.thinkersjournal.net`

Docs only; no code. Status: **options for CireSnave, awaiting a ruling.** Launch scope stays "people CireSnave names"
(admin-issued channels) until this is ruled.

## What CireSnave ruled (board 447, item 2, verbatim)

"ThinkersJournal's plan always included user-controlled services running on subdomains under ThinkersJournal.net.  On those
subdomains, users of ThinkersJournal.com should be able to surface services.  Services surfaced directly on ThinkersJournal.com
should be limited to those surfaced by ThinkersJournal's administrators."

So: users of ThinkersJournal.com can surface services on subdomains of `thinkersjournal.net`; the apex `thinkersjournal.com` is
reserved for services the administrators surface.

## What exists today (events mode, `docs/DESIGN.md`)

A sender POSTs `https://thinkersjournal.net/hooks/<channel-id>`; the channel id is an unguessable capability; a client on the
user's machine holds one WebSocket per channel. Channels are minted only by `POST /admin/channels`. There is no hostname per
service and no HTTP proxying (that is surface mode, roadmap row 6, `later`).

## The questions the note answers

### 1. Naming and allocation
Each service gets `<name>.thinkersjournal.net`. Names are lowercase DNS labels, 3 to 32 characters, first-come, with a reserved
list (`www`, `admin`, `api`, `abuse`, `mail`, `hooks`, `connect`, anything ThinkersJournal-branded, and look-alikes of those).
A released name is held for 30 days before reuse, so a stale link cannot be claimed by someone else. The apex
`thinkersjournal.com` and the bare `thinkersjournal.net` stay with the administrators.

### 2. Who may create one
A ThinkersJournal.com account in good standing, proved by the community's own sign-in (the community lane owns account
identity; SurfaceAnt consumes a signed assertion and never stores passwords). Terms acceptance and the 13+ rule
([`NOTICE-AND-TERMS.md`](NOTICE-AND-TERMS.md)) happen at creation. Each service must carry its own abuse contact (board 447,
item 1), defaulting to the account's verified email.

### 3. Per-person limits
Start small and raise on evidence: 1 service per account at first, 3 at most; a provisioning rate limit (for example 3 creations
per day per account and per IP); the existing per-service limits (1 MiB, 10 req/s) unchanged.

### 4. Abuse path
Same policy as events mode (no scanning, disable at once, board 119 for illegal content) plus a per-service report address, and
suspension of the **account's** services when one is disabled for abuse. A user subdomain can host content that looks like
ThinkersJournal, so it needs its own isolation: the Public Suffix List entry for `thinkersjournal.net` (so cookies and
storage cannot cross between users' services) and no cookies scoped to the parent domain. Surface mode must not ship before
both.

### 5. How it fits the relay channel model
A service is a channel plus a hostname. The Worker routes by `Host` (`<name>.thinkersjournal.net` -> the channel's Durable
Object) instead of by a secret id in the path. That changes the security model: the id stops being a capability, so a hostname
alone no longer authenticates a sender. Two consequences: (a) events mode keeps `/hooks/<id>` (the sender's own signature is
the trust, as today), and (b) surface mode (HTTP proxied to the user's machine) needs the exposure rules of row 6 (explicit
per-service opt-in, size and time limits, no cookies upward) written down first.

## Options

- **A (recommended): events mode first on user subdomains.** Add hostname-to-channel mapping, the name rules above, account-gated
  creation with the limits above, and the per-service abuse contact. Do not proxy HTTP yet. It delivers "users surface a service
  under a subdomain of their own" for webhook-style services with the abuse surface we already measured, and defers the hard
  part (proxying browsers' traffic) to row 6.
- **B: surface mode and user subdomains together.** One design and one launch. Delivers the full plan at once, but ships the
  largest abuse surface (arbitrary web content on a ThinkersJournal domain) with the least evidence.
- **C: administrator-only until later.** Keep every service admin-issued and put user subdomains behind a future row. Safest, but
  it postpones the thing CireSnave said the plan always included.

**Recommendation: A**, with the Public Suffix List submission and the community identity hand-off as prerequisites, and row 6 still
needing its own ruling. Needs from other lanes: the Community lane for the sign-in assertion and `[[LEGAL_ENTITY]]`; the
PM for the DNS zone and the Public Suffix List request (an external submission, made only on CireSnave's word).

-- lane: surfaceant
