# Row 5: abuse, privacy and terms decisions for CireSnave

Not decided here. Each item has a recommendation so he can answer with a letter; the PM relays and records the ruling. Facts
about the relay are from `docs/DESIGN.md` and the Worker at `origin/main` (2026-11-14). Items 1 and 2 gate any public launch;
none gates the deploy itself (row 4), which provisions only admin-issued channels.

## 1. Who operates the relay, and who receives abuse reports?
Terms, a privacy notice and an abuse contact all need a named operator. Board 129 already asks the legal entity name
(`[[LEGAL_ENTITY]]` in the Community lane's documents) and the portfolio has an `admin@` address.
- **A (recommended):** reuse the same entity and `admin@` address as ThinkersJournal; the relay's pages say so.
- B: a separate contact for SurfaceAnt.

## 2. Who can get a channel?
Today only a holder of `ADMIN_SECRET` can mint one (`POST /admin/channels`); there is no self-service path.
- **A (recommended):** admin-issued only at launch (people CireSnave names); self-service is a later row with its own limits
  (per-person channel cap, provisioning rate limit, email or GitHub identity).
- B: self-service from day one (needs an identity, a cap and an abuse path first).

## 3. Abuse handling
What the relay is exposed to: anyone who learns a channel id can POST up to 1 MiB (10/s, burst 50, then 429) to that user's
local program; the relay stores up to 24 h / 1000 events / 64 MiB per channel; it can host unwanted content only transiently.
Forged ids cost no storage (they 404 before reaching a Durable Object).
- **A (recommended):** no content scanning (we do not read payloads); the remedy is disabling the channel
  (`POST /admin/channels/<id>/disable`: immediate, sockets closed, no further events accepted), on a report to the abuse address or on sight of abuse
  by rate; a reporter's note is kept with the disable decision. Illegal-content reports (CSAM and the like) follow the existing
  ThinkersJournal reporting pipeline (board 119) rather than a new one.
- B: scan payloads (costly, and conflicts with item 4's "we do not read your data").

## 4. Privacy: what the operator can see and keep
The relay sees every payload in clear (TLS terminates at Cloudflare) and holds it in Durable Object storage until acked or
24 h; it keeps no body after that. Cloudflare's own request metadata and Worker logs exist while observability is on
(`wrangler.jsonc`).
- **A (recommended):** say so plainly in the notice: the operator and Cloudflare can read in-flight and buffered payloads;
  we never log bodies; metadata logs (time, channel id, status, size) kept 7 days then dropped; disabling a channel stops
  delivery at once, and its buffer is dropped at the 24 h limit (deleting it on disable is a small Worker change if wanted). End-to-end encryption between sender and client is not offered (it cannot work for GitHub-style senders that
  sign plaintext) and is documented as a non-goal.
- B: turn Worker logging off entirely (loses the CPU/error visibility row 4 relies on).

## 5. Terms of use
- **A (recommended):** a one-page terms: free, best-effort, no uptime promise, suspended at the operator's discretion,
  acceptable-use list (no malware delivery, no spam, no unlawful content, no traffic against others' systems),
  **no education records or children's personal data through the relay** until a data-protection agreement exists (the
  K-12 suite has FERPA/COPPA duties; SurfaceAnt is not built for them), 13+ users only.
- B: no separate terms; the privacy notice and abuse policy only (weaker: nothing to enforce suspension against).

## 6. Cost ceiling (decision, not a design question)
*On the board: items 1-5 are board 447; item 6 is board 445 (with the deploy).*

The Workers Free plan caps CPU at 10 ms per request and requests per day; row 4 measures real CPU. If p99 is close to the cap
or traffic outgrows the daily request quota the relay needs the Paid plan (about a flat monthly fee plus usage).
- **A (recommended):** deploy on Free, review after the first week of real numbers; upgrade only on evidence.
- B: start on Paid.

— lane: surfaceant
