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

## Nested names and where TLS ends

CireSnave asked (2026-11-16) how a user could get TLS for a nested name such as `mail.<user>.thinkersjournal.net` when a
visitor's TLS ends at Cloudflare. Facts first, each marked **verified** (read on the page cited, dated as shown on it) or
**UNVERIFIED**.

### Facts
| Fact | Status | Source |
|---|---|---|
| A DNS wildcard is multi-level: `*.example.com` covers `abc.example.com` and `123.abc.example.com`; only the first label may be `*`; an exact record at a name overrides the wildcard below it. | verified | [Wildcard DNS records](https://developers.cloudflare.com/dns/manage-dns-records/reference/wildcard-dns-records/) (updated 2026-04-16) |
| Universal SSL covers the apex and first-level subdomains only; `a.b.example.com` is "not covered and will not serve a valid certificate" on a full setup. | verified | [Universal SSL limitations](https://developers.cloudflare.com/ssl/edge-certificates/universal-ssl/limitations/) (2026-10-09) |
| Advanced Certificate Manager (ACM): up to 50 hosts per certificate (apex counts as one); a wildcard covers one level only, so `*.<user>.thinkersjournal.net` is a certificate of its own, and a deeper level needs another. | verified | [ACM](https://developers.cloudflare.com/ssl/edge-certificates/advanced-certificate-manager/) (2026-08-14) |
| ACM costs "starting at $10 per month" per zone on Free, Pro and Business. | verified from Cloudflare's plans page and launch post as returned by search; the docs pages themselves state no price, so **re-read the plans page before committing money** | [Plans](https://www.cloudflare.com/en-gb/plans/), [launch post](https://blog.cloudflare.com/advanced-certificate-manager/) |
| Cloudflare for SaaS: 100 custom hostnames included, $0.10 each after, up to 50,000, on Free, Pro and Business; **wildcard custom hostnames are Enterprise only**. | verified | [Plans](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/plans/) (2026-08-14) |
| A Workers route `*.thinkersjournal.net/*` matches multi-label hosts such as `a.b.thinkersjournal.net`. | **UNVERIFIED.** The route doc says `*` matches "zero or more of any character" and gives `*.sub.example.com/*` matching `foo.sub.example.com`, which implies it, but no page says it of a multi-label host. Test on a scratch name (a `scratch-<n>.thinkersjournal.net` route to a test Worker, request `a.b.scratch-<n>...`, record the result here, remove the route) before relying on it. | [Routes](https://developers.cloudflare.com/workers/configuration/routing/routes/) (2026-10-18) |
| Delegating a subdomain: "Cloudflare's CDN and security services are not applied to delegated subdomains" (caution box). Delegation is not possible on a partial (CNAME) setup. | verified | [Delegate subdomains](https://developers.cloudflare.com/dns/manage-dns-records/how-to/subdomains-outside-cloudflare/) (2026-08-14) |
| Glue (an `A`/`AAAA` for a nameserver) is required only when the nameserver's hostname is inside the delegated zone; for nameservers elsewhere it "needs no glue". Cloudflare serves glue it holds. Whether glue must be DNS-only is not stated. | verified (DNS-only is our inference: a proxied record answers with Cloudflare's addresses, not ours) | [Shadowed records](https://developers.cloudflare.com/dns/manage-dns-records/reference/shadowed-records/) (2026-07-06) |
| An NS record in a zone creates a delegation; records at or below it are "shadowed" (kept, not authoritative); Cloudflare returns no error when that happens. "If a record or delegation exists, the wildcard does not apply." | verified | same page; [Wildcard DNS records](https://developers.cloudflare.com/dns/manage-dns-records/reference/wildcard-dns-records/) |
| A **wildcard NS** record (`*.thinkersjournal.net NS ...`) delegates every user at once. | **UNVERIFIED.** Cloudflare's docs neither allow nor forbid it, and under the DNS standard (RFC 4592) a wildcard does not create a delegation, so resolvers would not follow it. Test on a scratch name before any design depends on it; the design below does not. | |
| `_acme-challenge` is a DNS TXT name (`_acme-challenge.<host>`), not an HTTP path. | from the ACME specification (RFC 8555), not re-read today | |
| HTTP-01 proves control of one named host by serving a token at `/.well-known/acme-challenge/<token>` on port 80, so it can be answered through a relay for a single named host, but a CA never issues a wildcard on HTTP-01; a wildcard needs DNS-01. | from RFC 8555 and the CA/Browser Forum rules, not re-read today | |
| Cloudflare offers SNI/TLS passthrough for HTTPS on the Free plan. | **not stated on any page read.** Spectrum is "available on Paid plans"; custom TCP/UDP applications need Enterprise plus the Spectrum add-on; Universal SSL is incompatible with Spectrum. | [Spectrum](https://developers.cloudflare.com/spectrum/) (2026-04-23) |

### Options
| | (a) One label + paths | (b) Cloudflare ACM or Cloudflare for SaaS | (c) Our own SNI-passthrough edge | (d) Per-user NS delegation |
|---|---|---|---|---|
| What the user gets | `<user>.thinkersjournal.net/mail/`; nested services are paths | `mail.<user>...` with a certificate per user: ACM `*.<user>...` at about $10/month per zone, or SaaS custom hostnames | Any name under `<user>.`; the user's software terminates TLS | The user's whole subtree on their own nameservers (one NS delegation per user in our zone) |
| Cost | none beyond the Universal SSL we have | ACM: about $10/month per zone, 50 hosts per certificate and one wildcard per level, so each user's `*.<user>...` takes one of a certificate's 50 slots (inference: about 49 users per certificate; the per-zone certificate cap on Free to Business is not stated on the pages read). ThinkersJournal pays. SaaS: first 100 hostnames free, then $0.10 each, no wildcard under Free to Business | Hosting and operating a public edge (below) | Free to us, hard to support |
| Who holds the keys | Cloudflare (edge) | Cloudflare | **The user, never leaves their machine**; certificate via an ACME-DNS-style API on our side (DNS-01, so wildcards work) or HTTP-01 through the passthrough | The user |
| Abuse control | Full: HTTP is visible, per-path rules and disable-at-once | Full at Cloudflare's edge, same as (a) | **Connection level only** (SNI, rate, bytes, disable by name); we cannot see or filter content, so board 447's "disable a channel at once" still works but content rules do not | Almost none; the user leaves our zone and policy |
| What we run | Nothing new | Certificate orders and renewal, per-user records | A new always-on TCP edge (not a Worker: a Worker cannot take a raw TLS stream; it would run on a small VM or container host with a public IP) with SNI routing to each user's connected client, and the ACME-DNS service. Capacity is connections held open, not requests, and bytes relayed: an estimate and a load test are needed before launch (no figure measured here) | Per-user NS records: Cloudflare allows at most 10 NS records per delegation name, and every record counts against the zone quota (200 on a Free zone created after 2024-09-01, 1,000 on older Free, 3,500 on Pro and Business); users also need a public IP |
| Fit with this design | Fits today's relay: the channel is already the unit | Fits, but cost grows with users | Large new component; defers surface mode's HTTP rules (the edge never reads HTTP) | Leaves Cloudflare, not recommended |

### Option (e): one delegated child zone, two VPS nameservers, a landing page per user
CireSnave's idea (2026-11-16; an idea, not a ruling). Cloudflare keeps hosting `thinkersjournal.net`. Two VPSs are the
authoritative nameservers for **one** fixed child zone, `s.thinkersjournal.net`, delegated once. Users' real services live in
it as `<service>.<user>.s.thinkersjournal.net`; `<user>.thinkersjournal.net` stays on Cloudflare as a Worker landing page.

1. **Why one fixed child zone, not a wildcard NS.** A wildcard NS is UNVERIFIED and, by the DNS standard, would not delegate
   (facts above). A delegation is exactly two NS records for `s` (no per-user records, so no record-count cap and no
   10-NS-per-name limit); the VPSs then synthesise every `<user>.s...` answer from our database.
2. **The label conflict.** A name cannot be both a Cloudflare-served landing page and a delegation (the NS at a name makes
   everything at or below it non-authoritative in Cloudflare). So the landing page is at `<user>.thinkersjournal.net` (one label:
   covered by Universal SSL; a proxied wildcard `*` record plus the `*.thinkersjournal.net/*` route serve it, and the exact
   `s` delegation overrides the wildcard for itself), and the services sit under `s`. `s` becomes a reserved name.
3. **Cloudflare's own caution applies.** CDN and security services are not applied to anything under `s`: no Cloudflare DDoS
   protection, no WAF, no proxy for the VPS addresses, which are public. Abuse and attack exposure is ours (see the table).
4. **What a VPS runs.**
   - An authoritative DNS server answering from our database (a user's name resolves to the relay edge, not the user's home).
   - An API for ACME DNS-01 TXT records: the user's software requests a wildcard certificate for `*.<user>.s.thinkersjournal.net`
     and holds the private key; we never see it. Wildcards need DNS-01, which is why we run the DNS.
   - Optionally an SNI-passthrough proxy (as in option c), so a user behind NAT works and the user's home IP is never published.
     **Recommend not showing a user's real IP anywhere**: a page that shows it invites harassment and attacks; show the relay name.
5. **NS registration.** Two nameserver hostnames, e.g. `ns1.thinkersjournal.net` and `ns2.thinkersjournal.net`, each an `A`
   record in the Cloudflare zone pointing at a VPS and kept DNS-only (inference above), then the two NS records for `s`. Because
   the nameservers are outside the delegated zone, Cloudflare requires no glue (verified above). The registrar is untouched:
   `thinkersjournal.net` itself stays on Cloudflare.
6. **The landing page** (a Cloudflare Worker, so it has Cloudflare's protection): a link to the user's public ThinkersJournal.com
   profile, an abuse-report form that feeds the normal ThinkersJournal.com abuse mechanism (board 119 for illegal content), and
   what the VPSs know about the service (name, created, online or not, relay name; never the user's IP).

| | (e) delegated child zone + 2 VPS |
|---|---|
| Cost | Two small VPSs (a hosting bill, no figure measured here) plus our time; Cloudflare itself costs nothing extra |
| Ops | Two public authoritative nameservers (patching, zone data replication, monitoring, an outage means every user's name stops resolving), a DNS-01 API, optionally a passthrough proxy; DNSSEC for `s` is its own decision |
| Keys | The user's; the certificate is issued to their software and the key never leaves their machine |
| Abuse control | Name-level (we can stop answering for a user or service at once) and connection-level if the passthrough runs; no content-level control; no Cloudflare shield in front of the VPS addresses |
| Risk | DDoS and amplification against two exposed IPs (rate-limit and response-policy needed), a single place whose outage affects all users, the wildcard certificate being issued for a name a user does not control (the API must check ownership) |
| Fit | Fits the one-delegation shape CireSnave described; the landing page keeps the human-facing identity on Cloudflare |

### Recommendation
**First launch is (a): one label, `<user>.thinkersjournal.net`, nested services as paths.** It needs no new certificate, no
passthrough and no money, and it keeps the abuse controls we ruled on. Treat (b) as the paid upgrade if users ask for nested
names and CireSnave accepts per-zone cost; test the Workers wildcard route first (the UNVERIFIED row). Keep (c) and (e) as later designs
of their own: they are the options where the user's keys never leave their machine, and (e) is the cheaper shape of it
(two fixed NS records instead of per-user delegation, and the landing page stays under Cloudflare's protection) but puts two
exposed nameservers in our hands. Either needs its own ruling on the loss of content-level abuse control and on who operates
the servers. Do not do (d).

-- lane: surfaceant
