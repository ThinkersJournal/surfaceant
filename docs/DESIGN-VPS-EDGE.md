# Design note: SurfaceAnt on our own VPS edge (`thinkersjournal.net` on two nameservers)

Docs only; no code. Status: **design for the PM and CireSnave to review, then build slice 1.**

## What CireSnave ruled (board 457, option F, 2026-11-17, as relayed by the PM)

`thinkersjournal.net` moves to CireSnave's two VPS nameservers; `thinkersjournal.com` stays on Cloudflare. The Cloudflare
Worker deployed on `.net` is **temporary** and will move under the `.com` account/zone until the VPS edge exists. CireSnave
accepts occasional outages "so long as nothing is permanently damaged". Mail is not in scope (a later decision).

This **supersedes** option (e) of [`DESIGN-SUBDOMAINS.md`](DESIGN-SUBDOMAINS.md) (a delegated child zone `s.thinkersjournal.net`
while Cloudflare keeps the rest). There is no `s` label now: the VPSs are authoritative for the whole zone, so a user's names
are `<user>.thinkersjournal.net` and `<service>.<user>.thinkersjournal.net`, at any depth. Cloudflare's Universal SSL and
route limits no longer apply to `.net`; options (c) and (e) there are the shape we are now building.

Facts below are marked **verified** (read on the page cited, with its date), **inference**, or **UNVERIFIED** (must be tested
in the slice named).

## 1. What runs on the two VPSs

Each box: 4 vCPU, 7.7 GB, Debian. Call them **A** (primary) and **B**. Box B also hosts staging, so everything runs in
containers (Podman or Docker; one compose project per environment) with these isolation rules:

- Staging is a separate compose project and network, with cgroup caps (for example 1 CPU, 2 GB) so a staging load test cannot
  starve production DNS. No volume or secret is shared between staging and production.
- Staging never binds port 53 or 443 on a production address. It uses an unprivileged port and a separate address or a private
  (WireGuard) network.
- Production containers on each box: `dns`, `relay` (box A only live; B warm standby), `edge` (TLS front, section 3), `page`
  (section 4), `acme-api` (section 3). Host firewall (nftables) allows only 53/udp, 53/tcp, 80, 443, SSH (key only, from
  known addresses) and WireGuard between A and B.

### 1.1 Authoritative DNS

- **Server: `hickory-server` (Rust).** Current stable on crates.io is 0.26.3 (2026-09-10; read from the crates.io API on
  2026-11-17). Why: it is Rust (our shop, our dependency rules), serves UDP and TCP, and takes a custom request handler, so we
  can **answer from our database instead of a zone file**. Alternatives (NSD, Knot DNS) are mature C servers; they stay the
  fallback if hickory cannot meet the tests in slice 1, and the design does not depend on the choice (the database and the
  tests are ours). **UNVERIFIED until slice 1:** that hickory's handler interface lets us synthesise answers per query, set
  our own response-rate limiting, and keep UDP responses small.
- **Why synthesise, not store a wildcard.** A static `*.thinkersjournal.net` record does not cover names *below* a name that
  exists (the "empty non-terminal" rule of the DNS standard, RFC 4592): once `alice.thinkersjournal.net` has any record,
  `x.alice.thinkersjournal.net` no longer matches the apex wildcard. We need every user name and any depth to resolve, with
  per-user exceptions (suspended names, reserved names, `_acme-challenge` TXT), so a handler that computes the answer from
  the database is simpler and correct for every depth.
- **Answers.** Apex: SOA, NS (`ns1`, `ns2`), A/AAAA of the relay edge, CAA (issuance limited to Let's Encrypt and our ACME
  account). Any other name: if its first label below the apex is a registered user (and not suspended): the edge's A/AAAA at
  every depth. Reserved names (`www`, `admin`, `api`, `abuse`, `mail`, `hooks`, `connect`, `ns1`, `ns2`, as in the subdomain
  note) are our own records. `_acme-challenge.<name>` TXT comes from the live ACME table. Unknown or suspended: NXDOMAIN
  (suspension is therefore a name-level kill switch). No recursion: `REFUSED` for queries outside our zone; `ANY` answered
  minimally.
- **Two nameservers, glue.** `ns1.thinkersjournal.net` and `ns2.thinkersjournal.net` are inside the zone they serve, so the
  registry needs **glue** (host records for both at the registrar). Registering the host records and changing the nameservers
  is CireSnave's act at Dynadot only. Prefer two different networks or providers so one outage does not take both.
- **Zone data.** From a **registry database** (section 1.2), not a zone file. Box A is the writer; box B pulls a signed
  snapshot over WireGuard every few seconds and serves it read-only. If A is down, B keeps answering from its last snapshot
  (names and addresses stay up; new users and ACME updates wait). Standard AXFR/NOTIFY cannot carry a synthesised zone (it
  cannot be enumerated), which is why the snapshot is ours.
- **Rate limiting and DoS posture:** per-source response-rate limiting (RRL), no large answers (cap UDP at EDNS 1232 bytes and
  truncate to TCP), a per-source token bucket on queries, caps on TCP connections and idle time, drop malformed packets before
  doing work, `ANY` and `DNSKEY` bulk answers disabled until DNSSEC exists. Public authoritative servers are amplification
  targets; RRL plus small answers is the control. See section 5.
- **DNSSEC.** Not in the first slices. Signing adds a key to custody and a failure mode (a bad signature or DS makes every
  validating resolver fail the whole zone). Decide after the zone has run unsigned; **if the Cloudflare zone currently has
  DNSSEC on, its DS record at the registrar must be removed before the cutover** (section 7).

### 1.2 State, in two stores

Two SQLite files (WAL), on box A, so the durable and the disposable never share a backup:

- `registry.db` (**durable**): users, services/channels (id, token hash, abuse contact, disabled flag), names and reserved
  list, suspensions, the audit log, and the live ACME TXT rows (short-lived). Replicated to B (snapshot) and backed up
  (section 5).
- `buffer.db` (**disposable**): buffered events with the 5-minute TTL (board 447). **Never backed up and never replicated**:
  it holds payloads, and the retention statement promises they are dropped after about 5 minutes.

## 2. The relay re-hosted as a server

Same wire protocol, so `surfaceant-client` does not change except for its relay URL. The re-expression of the Worker +
Durable Object model, piece by piece:

| Cloudflare model | On the VPS |
|---|---|
| Worker router (`/hooks/<id>`, `/connect/<id>`, `/admin/...`) | One Rust server (`axum` + `tokio`; `axum` is at 0.8.9 on crates.io, `tokio` 1.54.1) with the **same routes, status codes and bodies**: `404` for an unknown or forged id without touching state, `405` on other methods, `401`, `410` for disabled, `413`, `429` + `Retry-After` |
| One Durable Object per channel | One in-process **actor per channel** (a `tokio` task owning an `mpsc` mailbox and the live socket), created on first use; the actor serialises writes exactly as the DO did |
| DO SQLite `events` table | `buffer.db` table with `(channel_id, seq)`; `seq` is assigned by the channel actor inside one transaction (per-channel order, as today); `delivery_id` unique per channel gives the same idempotency |
| DO alarm (TTL sweep) | A sweeper task deleting rows older than `BUFFER_TTL_SECONDS` (default 300) every few seconds, plus per-channel cap enforcement (1,000 events or 64 MiB, oldest first, counted into the next `welcome` as `dropped_events`) |
| Hibernation API | Not needed: an idle WebSocket on our own server costs a socket and a little memory, not billed duration. Keep ping/pong so half-open sockets are found; a new connection still **replaces** the old one (`error{code:"replaced"}`) |
| Worker secrets `CHANNEL_KEY`, `ADMIN_SECRET` | Files read at start from the host's secret store (Podman/Docker secrets or systemd credentials), never in git, never in the image. `CHANNEL_KEY` is the HMAC key of every channel id: losing it invalidates all of them, so it is in the backup set (section 5) |
| Worker vars (`MAX_BODY_BYTES`, `RATE_PER_SEC`, ...) | Same names, as environment variables with the same defaults |
| Per-channel token bucket | In the actor, same numbers |

Re-used, not rewritten: `surfaceant-proto` (the frames) and `worker/src/logic.rs` (`make_channel_id`, `channel_id_is_valid`,
`hash_token`, `secret_eq`, `bearer`, `delivery_id_from`, `forward_header`, `abuse_contact_is_valid`), which depend only on
`hmac`, `sha2` and `subtle`, so they move to a shared crate used by both the Worker and the server. That also lets the
Worker keep working as the interim relay.

**Proof the protocol is unchanged:** the existing end-to-end example (`cargo run -p surfaceant-client --example e2e`: signed
sample, offline replay, duplicate, tampered-byte negative control, disable) and the shared test vectors must pass against the
new server with no edit to the client. That is the acceptance test of slice 3.

Failure behaviour: a restart loses nothing durable (registry is in SQLite; the buffer is in SQLite too and survives a process
restart, only the 5-minute TTL discards it); clients reconnect with `last_acked_seq` as today. A **box A outage** stops
delivery for its duration and drops buffered events past the TTL: accepted by CireSnave; senders see 5xx and retry (GitHub
redelivery is manual). Moving the relay to box B is a manual step (restore `registry.db`, start the container, switch the A
records, section 5); it is not automatic because two live relays would split a channel's state.

## 3. TLS

### 3.1 Our own wildcard `*.thinkersjournal.net` (and the apex), by DNS-01, locally

We are the authoritative server, so the ACME client writes the `_acme-challenge` TXT straight into `registry.db`; no DNS
provider API. A wildcard needs DNS-01 (verified: Let's Encrypt challenge-types page, last updated 2026-02-12: HTTP-01 and
TLS-ALPN-01 "cannot be used" for wildcards). Sequence: write the TXT, **wait until box B has picked it up** (otherwise the CA
may query the nameserver that lacks it), tell the CA, then delete the TXT. ACME client: `instant-acme` (0.8.5, 2026-02-24,
async Rust; **UNVERIFIED** that it fits the flow without change, checked in slice 4). Renew early (one third of the
lifetime left) and alert on failure; the key lives on the box, outside the image.

This certificate covers **one label**: `alice.thinkersjournal.net`, `www`, `hooks`, the apex. It does not cover
`mail.alice.thinkersjournal.net`.

### 3.2 Per-user wildcards for the user's own software (their key stays on their machine)

For a nested service (`mail.alice.thinkersjournal.net`) the user's software gets its own certificate for
`*.alice.thinkersjournal.net` from Let's Encrypt, using **its own ACME account and private key**; we never see either. We
provide only the DNS-01 helper, an **ACME-DNS-style API** on `acme-api`:

- The user's service token (the same bearer secret the client already holds) authorises `present` and `cleanup` calls in the
  shape of the `httpreq` DNS provider that common ACME clients ship (a POST with `fqdn` and `value`; **UNVERIFIED** against
  the exact current client specs, confirmed in slice 6), so unmodified clients work.
- It may write **only** `_acme-challenge.<user>.thinkersjournal.net` and `_acme-challenge.<label>.<user>.thinkersjournal.net`
  for the caller's own `<user>`. At most two live TXT values per name, each expiring after ten minutes. This is the ownership
  check the design needs: nobody can obtain a certificate for a name they do not control.
- Test against Let's Encrypt **staging** first.

### 3.3 SNI passthrough

The edge listens on 443 and reads the TLS ClientHello's SNI without terminating:

- SNI with **one** label below the apex (`alice.`, `www.`, `hooks.`, the apex): we terminate with our wildcard (the user page,
  events mode `/hooks/<id>`).
- SNI with **two or more** labels under a registered user: **passthrough**, a raw byte stream tunnelled to the user's connected
  client; the user's software terminates TLS with its own certificate. We see SNI, volumes and timing, never content.
- Unknown SNI: closed without a response. Encrypted ClientHello is not offered by us, so SNI is visible to route on.
- Port 80: redirect to HTTPS for names we terminate; nothing is tunnelled on port 80.

Passthrough needs a **stream tunnel** in the relay protocol (multiplexed streams over the client's existing WebSocket). That
is surface mode (ROADMAP row 6), reserved in `DESIGN.md` section 6 as the `kind: surface` frame set; it gets its own design
note and its own gate before slice 7. Until it exists, nested names resolve and our wildcard serves the one-label names only.

### 3.4 Let's Encrypt limits at launch size (read 2026-11-17 on the Let's Encrypt rate-limits page, last updated 2026-08-05)

| Limit | Value | Meaning for us |
|---|---|---|
| New certificates per registered domain | 50 per 7 days (global, all accounts); override by request form | The binding limit, see below |
| New certificates per exact identifier set | 5 per 7 days, no override | Do not loop a failing issuance; our own wildcard is one set |
| Failed authorizations per identifier per account | 5 per hour, no override | A broken helper retried in a loop locks the name for an hour: back off |
| New orders per account | 300 per 3 hours | Not a concern: each user has their own account |
| Accounts per IP | 10 per 3 hours | A user's own address; not ours |
| ARI renewals | exempt from all limits (the page: non-ARI renewals are exempt from the orders and registered-domain limits only) | Renewals do not use up the 50 |

"Registered domain" is determined from the Public Suffix List (the page says it uses the PSL and gives
`new.blog.example.co.uk` -> `example.co.uk`). **Inference, not stated on the page:** until `thinkersjournal.net` is itself on
the PSL, the registered domain is `thinkersjournal.net` for every user, so **all users' first issuances plus our own wildcard
share 50 per week**: about 50 new users a week, at most. After a PSL entry each `<user>.thinkersjournal.net` would be its own
registered domain and the limit stops being shared (**inference**, verify with Let's Encrypt before relying on it).

### 3.5 Certificates before a PSL entry exists (PM [TASK] interim-until-psl, CireSnave 2026-11-17)

CireSnave accepted building the service and getting users first, then applying to the PSL. Until the entry exists the 50 a
week in section 3.4 is shared by every user, so the plan beyond it is, in order:

1. **Count first.** Our own wildcard is one issuance and renewals are exempt, so the budget is about 49 new per-user
   wildcards a week. Only users who run their own nested services (section 3.2) need one; events mode and the one-label page
   use our wildcard and cost nothing against it. Track issuances per week in the usage log (section 8.2) and alert at 40.
2. **Ask Let's Encrypt for the rate-limit adjustment.** The rate-limits page offers a request form for the registered-domain
   limit and says it takes a few weeks, so file it **at launch, not when the limit is hit**. An answer is not guaranteed
   and the page does not say the PSL status matters; treat as **UNVERIFIED** until it answers.
3. **Fallback CA with its own limits: Google Trust Services Public CA**, read 2026-11-17 on its own Cloud documentation
   (`docs.cloud.google.com/certificate-manager/docs/quotas`, no last-updated date shown): ACME, DNS-01 and wildcards are
   supported and free of charge; the quotas are 100 new orders per hour, 100 new accounts per hour (25 per minute), 300
   authorizations per hour, **per Google Cloud project**, adjustable by request, and the page states no per-registered-domain
   limit and does not mention the PSL. Costs: **external account binding is required**, each ACME account is bound to a
   Google Cloud project by an EAB key (one key, one account, used within 7 days), and a project's accounts share its quotas.
   So the fallback fits **our own** issuance and a user whose software we provision; it does **not** let each user's
   unmodified client get a certificate without a Google Cloud project of their own. CAA at the apex must then also allow
   `pki.goog` (section 1.1 lists CAA). Absence stated with its control: the page states no per-domain limit (the same page
   states the per-project ones, so the page was read, not skipped).
4. **ZeroSSL and SSL.com were not verified**: their official documentation pages returned HTTP 403 and 404 to our fetch on
   2026-11-17, so no claim about their limits is made. Read them by hand before naming either.
5. Beyond that, new nested-service users **queue** (a waiting list shown on the user page), rather than failing: events mode
   and the page keep working without a certificate of their own.

## 4. The per-user page

Served by `page` at `<user>.thinkersjournal.net`, terminated with our wildcard. Static content built from the database and
escaped; **no user-supplied HTML, no cookies, no scripts from other origins**, a strict CSP, `Referrer-Policy: no-referrer`.

- Name, created date, online or offline, the relay's name, a link to the user's **public ThinkersJournal.com profile**.
- An **abuse-report form** that feeds the normal ThinkersJournal.com abuse mechanism (illegal content follows board 119, not a
  new pipeline). The page posts to an endpoint the Community lane owns; the interface (URL, auth, fields) is a dependency to
  agree before slice 5. The form is rate limited per source and per user; the reporter's address is held only in memory for
  the limiter.
- **Never shows a user's real IP address**, or anything derived from it, and the relay never sends it to any page: a user's
  client connects outward to us, and every public record points at the edge. The service's own abuse contact (board 447) is
  shown beside ThinkersJournal's.

## 5. Abuse, DoS, backup and restore

### 5.1 Posture

| Threat | Control | Residual |
|---|---|---|
| DNS amplification / reflection | RRL, small answers, no `ANY`, no recursion, EDNS 1232 | A flood still fills a box's link; the provider may null-route it: an outage, accepted |
| Volumetric flood on two public IPs | Provider-level protection if offered; short outages accepted ("so long as nothing is permanently damaged") | **No Cloudflare shield on `.net` any more**: this is the cost of option F |
| TLS/TCP exhaustion, slowloris | Per-source connection caps, ClientHello and idle timeouts, drop unknown SNI | Same |
| Webhook floods to `/hooks/<id>` | Existing limits (1 MiB, 10 req/s burst 50 -> 429, caps) plus a per-source limit at the edge | |
| Abusive user content | We cannot read passthrough traffic. Kill switches, at once and without a deploy: stop answering for the name (NXDOMAIN), deny the SNI, drop the tunnel, revoke the ACME token, suspend the account's services; the policy text is in `NOTICE-AND-TERMS.md` | A certificate already issued stays valid, but the name no longer resolves |
| Compromise of a box | Containers, firewall, no secrets in images, key-only SSH, secrets in the host store; box B has staging, so keep it apart | Rotate `CHANNEL_KEY` only by reissuing every channel id |

Logging as ruled: status, channel-id hash and size; never bodies or headers; metadata kept 7 days.
**`NOTICE-AND-TERMS.md` section 3 must change at cutover**: it says traffic is encrypted to Cloudflare. After the move the
operator terminates TLS for first-label names (and can read events), and cannot read passthrough traffic.

### 5.2 What is backed up

| Item | Backed up | Why |
|---|---|---|
| `registry.db` | Yes: SQLite online backup every 15 minutes, encrypted (age), copied to the other VPS and to one off-provider store, 14 daily and 4 weekly kept | Users, channels, token hashes, names: irreplaceable |
| Secrets: `CHANNEL_KEY`, `ADMIN_SECRET`, the ACME account key (and DNSSEC keys if ever used) | Yes, in CireSnave's custody store as `DEPLOY.md` already describes, not in the database backup | Losing `CHANNEL_KEY` invalidates every channel id |
| Certificates and private keys | No | Reissued in minutes by DNS-01 |
| `buffer.db` | **No** | Payloads; the 5-minute retention promise |
| Compose files, config, the container build | In git | |
| The domain registration and registrar account | n/a | CireSnave's; expiry is a "permanently damaged" risk, so auto-renew and a calendar alert |

### 5.3 Restore

Target: **recovery point 15 minutes, recovery time about an hour, by hand.** Steps: provision a Debian box and the compose
project from git; restore secrets from custody; decrypt and restore the latest `registry.db`; start `dns`, then `relay`;
change the relay A/AAAA (low TTL) if the address changed; check with `dig` from outside and the e2e example. A **restore drill
once a month** onto the staging box, the e2e example as the pass test: an untested backup is not one. If box A is lost, B
already serves DNS from its last snapshot; the manual steps above promote B.

## 6. Out of scope

Mail (a later decision). Self-service signup (still admin-issued channels; launch scope stays people CireSnave names). HTTP
content proxying by the edge (we do not read passthrough traffic; surface mode is row 6).

## 7. Cutover and rollback

The Cloudflare zone for `thinkersjournal.net` is **kept intact as the fallback**. The nameserver change at Dynadot is
**CireSnave's act only**; no lane touches it.

Before any change:

1. Export every record of the Cloudflare `.net` zone (dashboard or API, by a person with the access; no lane has the
   credentials). **Check DNSSEC**: if enabled, remove the DS at the registrar *first* and wait out its TTL, or validating
   resolvers will fail the whole zone after the move.
2. Slice 1's static zone on A and B must answer **identically to Cloudflare for every exported name** (a diff script over
   `dig`, run against all three). Then the DB-backed synthesis must match the same results.
3. The relay must already be running and passing e2e on the VPS (slices 3-4), and the interim Worker moved under the `.com`
   account/zone. **Open for the PM:** the Worker's hostname and hook URL on `.com` differ from today's
   `https://thinkersjournal.net/hooks/<id>`, so senders (GitHub webhooks) and clients need the new URL, and
   `DEPLOY.md` and the README change.
4. Rehearse the whole sequence on a scratch domain CireSnave is willing to move (an open question below).

The move:

5. CireSnave registers the glue host records for `ns1` and `ns2` at Dynadot, then switches the nameservers.
6. Watch from several public resolvers: SOA serial, a user name, NXDOMAIN, TCP, the relay's hook URL, certificate validity.
   Resolvers cache the old NS set for up to the parent's TTL (typically up to 48 hours, set by the registry; **UNVERIFIED for
   `.net`, read it from the registry's delegation**), so both sets must stay identical for that window: **every record change
   is written to both Cloudflare and the VPS until decommission**.
7. Keep Cloudflare's zone as fallback for at least two weeks of clean running before anything is removed.

Rollback (any time before step 7): CireSnave switches the nameservers back at Dynadot; because both sets were kept identical,
the caches of either are correct. Triggers: DNS errors above a few percent for ten minutes, certificate failure on the hook
host, or the relay failing e2e against the real name. After rollback, the interim Worker keeps serving `.com`.

## 8. Public Suffix List

`thinkersjournal.net` must be on the PSL's private section before any public user-controlled web content, so one user's
cookies cannot reach another's and so Let's Encrypt limits stop being shared (section 3.4). From the PSL guidelines page
(last edited 2025-05-23):

- The **domain owner's authorised representative submits** (a pull request to `publicsuffix/list`); "we will not accept
  patches submitted by third party users". That is CireSnave or the legal entity, not a bot lane.
- Put a `_psl` **TXT** record at `_psl.thinkersjournal.net` whose value is the pull-request URL, and **leave it in place**
  after validation. It is served by our own DNS.
- The domain must have **more than two years of registration left** at submission, with a commitment to keep it so: check the
  Dynadot expiry first.
- The description gives why, the expected input and output, wildcards used, and the number of domains; run the list's
  `make test`; the entry is sorted by company name; follow the ticket until closed.
- **Risk: it can be refused until there are users.** The page says projects "not serving more then thousands of users are
  quite likely to be declined" and small, temporary, lab or beta projects "will likely be declined"; there is no service level
  on time and downstream copies lag. Plan for refusal at launch.

If refused or pending: launch stays events-only, one label (our wildcard, no user-controlled web content), people CireSnave
names; no per-user wildcards beyond the 50-a-week budget (section 3.5); surface mode and passthrough wait. Submitting is an
external act made only on CireSnave's word.

### 8.1 Interim isolation until a PSL entry exists (PM [TASK] interim-until-psl, CireSnave 2026-11-17)

CireSnave accepted: *build the service and get users first, then apply to the PSL* (relayed by the PM). The risk this
section closes: without the entry, a service on `alice.thinkersjournal.net` can set a cookie with `Domain=thinkersjournal.net`
and bob's service on `bob.thinkersjournal.net` receives it (cookie tossing, session fixation, login-CSRF).

**Gate, kept visible: surface mode (row 6) must not ship before the PSL entry exists and no parent-domain cookies are
possible.** Everything below is what may ship *before* the entry, and nothing in it relaxes the gate.

| Mode before the entry | Ships? | Why |
|---|---|---|
| Events/webhook mode (no browser; a sender POSTs to `/hooks/<id>`, the user's client reads the stream) | **Yes** | No browser renders user-controlled content on a user name, so there is no cookie jar to poison |
| Named users (people CireSnave names, per the paragraph above) | **Yes** | Trust is personal until the entry exists |
| Our terminated one-label names (page, hooks, apex) | **Yes, with the strip rule below** | We write every byte served, and the page already sets no cookies (section 4) |
| Passthrough / surface mode (user's own software behind our SNI route) | **No, waits for the entry** | We cannot see inside a passthrough stream, so we cannot strip a header from it; no edge rule protects it |

**Strip rule (where the edge terminates TLS):** the edge removes the `Domain` attribute from every `Set-Cookie` response
header it emits for a name under `thinkersjournal.net`, so any cookie it ever sets is host-only. Match the attribute
name case-insensitively, tolerate whitespace around `=` and `;`, apply to **every** `Set-Cookie` header on the response (there
may be several), leave other attributes untouched, and drop no cookie, only the attribute. It protects only responses we
terminate; it says nothing about passthrough, which is why passthrough is in the table above as No. Defence in depth: the
page sets no cookies at all (section 4); any cookie we introduce later uses the `__Host-` prefix, which browsers refuse
unless it is host-only, secure, path `/` and carries no `Domain`.

**Test (slice 5, fails if the rule is wrong):** a unit test on the header filter with inputs
`a=b; Domain=thinkersjournal.net; Path=/` -> `a=b; Path=/`; `a=b; domain = .thinkersjournal.net; Secure` -> `a=b; Secure`;
`a=b; Domain=alice.thinkersjournal.net` -> `a=b` (a subname is stripped too: host-only is the only form); two `Set-Cookie`
headers both stripped; a cookie with no `Domain` unchanged; and an HTTPS integration test against the staging edge that
requests a handler response containing `Domain=` and asserts the wire header has none. A mutation (rule disabled) must make it
fail; run it and read the failure, not just the pass.

### 8.2 Usage log from day one

The PSL maintainers ask for the number of people using subnames, so the registry keeps, from the first user:

- `distinct_holders`: the count of **distinct people who hold a subname** (an account holding three names counts once), stored
  as one row per day: `(date_utc, distinct_holders, names_total, le_issuances_this_week)`. Numbers and a date only, **no
  personal data** and no way to go from a row to a person; it is computed from the registry and the registry is not exported
  into it.
- Written daily by the edge process, included in the snapshot to box B and in the off-provider backup (section 5.2), and
  read out by a small admin route (`GET /admin/usage`, behind the existing admin secret) so the PM or CireSnave can quote the
  figure and its date to the PSL ticket without database access.
- The PSL guidance (section 8) says projects "not serving more then thousands of users are quite likely to be declined"; this
  log is how we know when the figure is worth submitting, and what number we can honestly state. Slice 2 creates the table,
  slice 3 the route.

## 9. Build order, smallest slice first

Each slice is its own PR (one PR per day per project; several slices may share a PR when done together) and has a test that
fails if it is wrong.

1. **DNS server serving a copy of the zone** (built 0.0.19 against fixture zones; the real export, the Cloudflare diff and the items listed under *Slice 1 results* are still open). A new crate `surfaceant-edge` with `hickory-server` loading a static export of
   the zone; UDP and TCP on a high port in CI. Test with `dig`: apex SOA and NS, a name from the export, NXDOMAIN, a wildcard
   case, TCP, truncation to TCP for a large answer, `REFUSED` for an out-of-zone name. Runs on A and B with the diff script
   against Cloudflare. Also settles the UNVERIFIED hickory claims in section 1.1 (or switches to NSD/Knot).
   *Slice 1 results (0.0.19, hickory-server 0.26.3, fixture zones only).* **Verified:** it serves UDP and TCP from a master file; apex SOA, NS, A, CAA; NODATA with SOA in authority; NXDOMAIN with AA; a record that exists beats the wildcard; a direct-child wildcard answer is owned by the queried name; a 4-record 1 KB TXT is truncated (TC, reply at most 512 bytes) on UDP without EDNS and whole on TCP; `REFUSED` out of zone on both transports. **Finding:** its file store applies `*.apex` at *any* depth, including below a name that exists (`x.alice` gets the wildcard address). RFC 4592 says NXDOMAIN there, and Cloudflare follows the RFC, so a static copy and Cloudflare differ for such names; the diff script in section 7 will show it, and slice 2's per-query synthesis is the fix, as section 1.1 argues. A test pins the deviation so a hickory upgrade that fixes it is noticed. **Still UNVERIFIED:** our own response-rate limiting, the EDNS 1232 cap, `ANY` handling, and a handler that synthesises per query (slice 2).
   **KNOWN DIFFERENCE from Cloudflare (hickory-server 0.26.3, found in slice 1; PM 2026-11-17: keep it visible).** Over a zone with an apex wildcard, a name below an existing name (`x.alice.<zone>` when `alice` exists) gets the wildcard's address from hickory's file store; RFC 4592 section 3.3.1 and Cloudflare answer NXDOMAIN. A name that resolves on one server and not the other is a cutover regression in either direction, so the section 7 diff must cover names at depth two and deeper, and slice 2 must either synthesise per query (our plan) or carry a patched store; until then no static copy is served for real. Pinned by the test `hickory_file_store_wildcard_matches_at_any_depth_unlike_rfc4592` in `crates/surfaceant-edge/tests/zone_file.rs` (fails when an upgrade fixes it). **Upstream:** already reported as hickory-dns/hickory-dns#3893 (open, 2026-09-06: the climb passes existing names; the same issue also lists a type-miss taking the wildcard and a NODATA returned as NXDOMAIN, which we have not reproduced), so we filed nothing; #1342 is a different report and #1275 is unrelated.
2. **Database-backed answers**: `registry.db`, synthesis at any depth, reserved and suspended names, RRL, snapshot to B, the daily usage-log table (section 8.2).
3. **Relay server** with the Worker's routes and limits on `registry.db` and `buffer.db`, **plus backups and the first restore
   drill** (before anything depends on it). Pass the existing e2e unchanged.
4. **TLS** for our wildcard by local DNS-01; the relay served over HTTPS on staging, then on the real names.
5. **User page and abuse form** (needs the Community lane's endpoint), and the `Set-Cookie` `Domain` strip rule with its test (section 8.1).
6. **ACME API** for per-user wildcards, against Let's Encrypt staging.
7. **Stream tunnel and SNI passthrough** (row 6's own design and gate; needs the PSL decision).
8. **Cutover** (section 7), after the rehearsal.

## 10. Direct mode (design; docs only; PM [TASK] design-direct-mode, 2026-11-17)

CireSnave asked whether a user's names could be answered with the user's own stable IPv6 (`AAAA`) so most traffic skips the relay, and whether a rotating IPv4 could be published as the primary with the relay as fallback. Facts are marked **verified** (source named, read 2026-11-17), **measured** (by us, on the stated machine), or **UNVERIFIED**. Quotes are short and verbatim from the cited page. Source list at the end of this section.

**Short answers.** (1) `AAAA` to the user plus `A` to the relay works for dual-stack clients, with a failover cost of roughly one connection-attempt delay (about 0.2 to 0.3 s) in the clients we could check, and leaves IPv4-only clients on the relay. Two gaps need decisions: an IPv6-only client has no relay fallback, and browsers and phones are UNVERIFIED. (2) DNS has no "primary A": order is not significant, and clients differ from 0.2 s to 21 s when the first address is dead; the workable form is our server publishing the user's IPv4 only while a probe succeeds. (3) Direct mode only works for users who pass four checks in 10.3. (4) The security rules in 10.4 are the part that must not be skipped. (5) It must be opt-in (10.5). (6) Build AAAA-direct first (10.6).

### 10.1 `AAAA` to the user, `A` to the relay

- **What a dual-stack client does (verified).** RFC 8305 ("Happy Eyeballs v2"): the client sorts addresses by RFC 6724, interleaves families, starts the first attempt, and starts the next after a delay: "One recommended value for a default delay is 250 milliseconds." (min 100 ms, max 2 s); attempts are staggered, "one at a time", and unattempted addresses are ignored once one connection succeeds. So a dead or filtered user `AAAA` costs a dual-stack client about one delay before it connects to the relay `A`. A target that refuses or is unreachable at once costs less.
- **IPv4-only clients (inference from the above).** They never ask for or use the `AAAA`; they get the relay. That is the fallback working as intended.
- **IPv6-only clients: no fallback (inference, UNVERIFIED on real networks).** A client that can only use IPv6 gets the user's `AAAA` and nothing else from us (a DNS64 resolver on a NAT64 network would synthesise an address from the relay's `A`, but only when no `AAAA` exists). If the user's IPv6 is down, that client fails. Mitigation: publish the user's `AAAA` only while the probe in 10.4 succeeds, and otherwise publish the relay's `AAAA` (the relay boxes need IPv6 addresses for this).
- **Per client class** (what the documentation says, and what we measured; we did **not** measure a browser, Go, a phone or Rust):

| Client | Documented behaviour | Measured by us |
|---|---|---|
| curl / libcurl | Happy Eyeballs, `CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS` default 200 ms; "If there are only addresses for one stack, those are tried one after the other"; whole attempt capped by the connect timeout (default 300 s) | curl 8.21.0, Windows: dead first `A` (`192.0.2.1`) then a live `A`: **0.22 s** (3 runs, 0.215 to 0.226 s; control, live only: 0.014 s). Dead only: still trying at our 20 s cap |
| Go `net.Dialer` | `FallbackDelay` default 300 ms for RFC 6555 fallback; "Dial will try each IP address in order until one succeeds" (from a model summary of the page; the middle of the page was not read in full) | not run |
| Rust `std::net::TcpStream::connect` | "If `addr` yields multiple addresses, `connect` will be attempted with each of the addresses until a connection is successful." Sequential; no parallel attempt | not run (the OS connect timeout decides) |
| Rust `hyper-util` `HttpConnector` (reqwest uses hyper) | `happy_eyeballs_timeout` default 300 ms; `None` disables parallel attempts | not run |
| Python `socket.create_connection` | sequential per address | dead first `A` then live: **21.03 s** stall (Windows SYN timeout) |
| Chrome, Firefox | the curl page says "Firefox and Chrome currently default to 300 ms" | not run |
| Safari/Apple, Android, OkHttp | **UNVERIFIED** (RFC 8305's authors are at Apple, which is not evidence of what the frameworks do; the OkHttp page returned 404) | not run |

  The host used for the measurements has **no working global IPv6** (a request to an IPv6-only site failed), so the dead `AAAA` attempt failed within about 15 ms (the connection went to the live `A` at once): that run shows only the fast-fail case, not a black-holed `AAAA`. A true black-holed `AAAA` test needs a host with IPv6 and a drop rule; do that before enabling direct mode for anyone.
- **In-flight connections do not move (inference from TCP).** A connection already open to the user's address breaks if it goes down; the client reconnects and, after the DNS change, gets the relay.

### 10.2 "Primary `A` / secondary `A`" and health-checked `A`

- **DNS has no priority (verified).** RFC 1034 section 3.6: "The order of RRs in a set is not significant, and need not be preserved by name servers, resolvers, or other parts of the DNS." RFC 6724 says an application "would then typically try the first address in the list, looping over the list" and that DNS order is only the last tie-break after the destination-selection rules; RFC 1794 describes round-robin rotation and says "Clients, of course, may reorder this information". BIND's default is to return records "in cyclic order" (BIND ARM `rrset-order`). So a "primary" `A` is a hope, not a rule.
- **What our own server does (measured).** hickory-server 0.26.3 returned the two `A` records of `multi.fixture.test` in the same order in 8 of 8 queries (no rotation); test `two_a_records_are_one_rrset_with_no_priority`. A resolver or client in between may still reorder.
- **What a dead first address costs:** 0.2 s (curl, parallel after its timer) up to 21 s (Python, sequential, Windows) in the two clients we measured; a sequential client with a long OS connect timeout is the bad case. Two `A` records make failover client-dependent, so we do not use them.
- **The alternative: one `A`, chosen by health.** Our authoritative server answers the user's IPv4 only while our probe of it succeeds, else the relay's, with a short TTL. Failover time is about (probe interval times failures needed) plus the TTL that resolvers keep, plus in-flight connections breaking.
- **TTLs resolvers keep (verified, partly).** RFC 2181 section 8: "The TTL specifies a maximum time to live, not a mandatory time to live", and implementations "are always free to place an upper bound". Unbound: `cache-max-ttl` default 86400 s, `cache-min-ttl` default 0 (disabled). BIND: `max-cache-ttl` default 604800 s, `min-cache-ttl` default 0 and cannot exceed 90 s. So a well-behaved resolver does not hold a 30 s TTL longer than 30 s; a resolver that sets a floor can (at most 90 s in BIND). Google Public DNS, Cloudflare 1.1.1.1, ISP resolvers, browser and OS caches: **UNVERIFIED** (their pages say nothing about TTL clamping). RFC 8767 serve-stale lets a resolver answer from an expired entry (suggested 30 s TTL on stale answers, 1 to 3 days max stale) when the authoritative server is unreachable: it does not apply while we answer, but it means our own outage is masked for a while.
- **Recommendation:** TTL 30 s on direct-mode names, probe every 10 s with three misses to withdraw, so expect about 1 minute to failover for well-behaved resolvers (a target to measure, not a promise). A shorter TTL multiplies query load on the two boxes; the usage log (section 8.2) will show it.

### 10.3 What must be true on the user's side

1. **Inbound must be allowed.** Residential IPv6 gateways are expected to block unsolicited inbound traffic: RFC 6092 section 2.3: "transport layer traffic is not forwarded into the interior network of a residential IPv6 gateway unless it has been solicited explicitly by interior transport endpoints". RFC 7084 does not state a default for IPv6 (UNVERIFIED beyond that). The user must open the port on their router (a firewall rule, or PCP where supported), so direct mode is not zero-setup.
2. **IPv4 behind carrier-grade NAT cannot receive inbound.** RFC 6598: shared space is `100.64.0.0/10`, "not globally routable"; section 5.2: "some peer-to-peer applications cannot seed content due to the inability to open incoming ports through the CGN". A user whose router's WAN address is in `100.64.0.0/10` (or private) is relay-only for IPv4.
3. **A stable address.** RFC 4941 temporary addresses are for "initiating outgoing sessions", with default preferred lifetime 1 day and valid lifetime 1 week, and may be preferred over public addresses for outbound; RFC 7217 gives a stable (per-network) interface identifier. A prefix delegated by the ISP can change (UNVERIFIED: no source read; ISP behaviour varies). So the agent must report a **stable** address and re-report on change; the record follows it (10.4).
4. **A certificate valid on both paths.** On the relay path the edge either terminates with our wildcard (one label below the apex, section 3.1) or passes TLS through (deeper names, section 3.3). On the direct path the user's own software terminates TLS, so it needs its own certificate for the name (their wildcard by the ACME-DNS design, section 3.2, or a certificate for the single name; mind the section 3.5 rate limits). If both paths terminate with different certificates, both must be valid for the name; the simplest rule is **passthrough on the relay path too**, so one certificate serves both and the relay never sees the plaintext. That requires the stream tunnel (slice 7) for the fallback to carry anything but events.

### 10.4 Security: a user-controlled `A`/`AAAA` is an attack surface

The agent never sends us an address to publish. Rules, all enforced by the server:

1. **Source-address rule.** Publish only the address our edge saw as the source of the agent's authenticated control connection (the existing bearer token). The agent must connect to us **from its stable address** (bind it explicitly): a default outbound connection uses a temporary address, which is not the address that will accept inbound (10.3 item 3). A report whose claimed address differs from the observed source is ignored.
2. **Global unicast only.** Reject anything that is not globally reachable: IANA special-purpose registries list `Globally Reachable = False` for `10/8`, `172.16/12`, `192.168/16`, `100.64/10`, `127/8`, `169.254/16` (includes the cloud metadata address `169.254.169.254`), `::1/128`, `fe80::/10`, `fc00::/7` (AWS also uses `fd00:ec2::254`); also reject multicast, our own box addresses, and any address already published for **another user** (victim-pointing: a name that sends a visitor's browser to someone else's host). The rules cover the DNS-rebinding family of attacks that Private Network Access describes (public names resolving to local addresses); browser defences exist (Chrome's preflight, July 2022 blog; current enforcement status UNVERIFIED) but we do not rely on them.
3. **Prove inbound and ownership with a probe.** Before publishing, we connect to the address and port and require a response only the agent can give (a nonce signed with its token). That proves the address is reachable, is the agent's, and is not the user's private address behind NAT; it is repeated every probe interval (10.2).
4. **Authenticated and rate-limited updates.** One address per family per name, at most N changes per hour per service, every change in the audit log with the observed source.
5. **Removal.** The record is withdrawn when probes fail (the name then answers the relay), when the service is disabled or suspended (NXDOMAIN, section 1.1), or when the user turns direct mode off.

### 10.5 Privacy: recommendation for CireSnave (his decision)

The design promises users' real IPs are not shown; direct mode publishes them in public DNS, where anyone can read them, and passive-DNS databases may keep history (UNVERIFIED: not looked up). **Recommendation: opt-in per user and per name, default relay-only,** with this consent sentence shown before it is switched on and recorded with a timestamp: *"Direct mode publishes this computer's internet address (IP address) in public DNS for <name>. Anyone can see it, learn roughly where you are and which provider you use, and send traffic straight to your computer; we remove it within about a minute of switching direct mode off or your agent going offline, but copies others have kept stay."* `NOTICE-AND-TERMS.md` section 3 gets a matching paragraph at the same cutover edit.

### 10.6 Build order for direct mode

Needs slice 2 (database-backed answers), slice 3 (relay), and slice 4 (TLS) first.

1. **AAAA-direct (no failover logic).** `registry.db` gains a per-name direct address (family, address, consent timestamp, verified_at); the agent reports its stable address over the control connection; the edge applies 10.4 rules 1 to 3; the synthesiser answers the user's `AAAA` plus the relay `A` (and the relay `AAAA` while the probe fails). Tests: every rejected range, a mismatching claimed address, a failed probe, consent absent, removal on probe failure.
2. **Measure before enabling for anyone:** a real black-holed `AAAA` on a host with IPv6, across a browser, curl, Go and Rust clients and a phone, recorded in this section replacing the UNVERIFIED cells.
3. **Health-checked IPv4 (only if wanted):** same record table for `A`, plus the TTL and failover numbers from 10.2 measured against real resolvers.

### Sources (read 2026-11-17)

RFC 8305 (Dec 2017) sections 3, 4, 5, 8; RFC 6724 (Sep 2012) section 2, 6; RFC 1034 (Nov 1987) section 3.6; RFC 1794 (Apr 1995); RFC 2181 (Jul 1997) section 8; RFC 8767 (Mar 2020) sections 4, 5; RFC 6092 (Jan 2011) section 2.3; RFC 7084 (Nov 2013); RFC 6598 (Apr 2012) sections 5.2, 7; RFC 4941 (Sep 2007) sections 3, 5; RFC 7217 (Apr 2014); IANA IPv4 and IPv6 special-purpose address registries (last updated 2025-10-09); curl `CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS` and `CURLOPT_CONNECTTIMEOUT_MS` pages (curl.se/libcurl/c); Rust `TcpStream::connect` docs (std 1.100.0, 2026-11-09); `hyper-util` 0.1.21 `HttpConnector` docs (docs.rs, 2026-09-24); Go `net` docs (pkg.go.dev, go1.27.4, 2026-11-10); Unbound `unbound.conf(5)` and the BIND 9 ARM (readthedocs, "latest"); WICG Private Network Access draft (2024-09-26) and the Chrome PNA preflight blog (2022-07-07); AWS EC2 instance-metadata docs. Our measurements: curl 8.21.0 and Python on Windows 11, 2026-11-17, against a local HTTP server and `192.0.2.1` (TEST-NET-1) as the dead address; hickory-server 0.26.3 on the fixture zone.

## Open questions

- **PM:** the Worker's temporary hostname on `.com` and the hook-URL change; whether the abuse-form endpoint can be agreed
  with the Community lane now.
- **CireSnave:** the VPS providers and addresses (two networks?); Dynadot expiry of `thinkersjournal.net` (the PSL needs more
  than two years); the off-provider backup destination; a scratch domain for the rehearsal; DNSSEC yes or no; who restores
  and who is told during an outage.

-- lane: surfaceant
