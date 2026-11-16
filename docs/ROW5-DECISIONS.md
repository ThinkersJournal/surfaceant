# Row 5: abuse, privacy and terms: CireSnave's rulings (board 447)

Ruled by CireSnave 2026-11-16, relayed by the PM. His words, verbatim: "447: 1: A...but there should be an abuse contact listed
for each service in addition to an abuse contact for SurfaceAnt as it relates to ThinkerJournal as a whole.  2: ThinkersJournal's
plan always included user-controlled services running on subdomains under ThinkersJournal.net.  On those subdomains, users of
ThinkersJournal.com should be able to surface services.  Services surfaced directly on ThinkersJournal.com should be limited to
those surfaced by ThinkersJournal's administrators.  3: A.  4: Content to be relayed should only be kept for long enough for a
service that is online to receive it.  I would think 5 minutes would be sufficient.  That said, A.  5: A."

| # | Question | Ruling | Where it lands |
|---|---|---|---|
| 1 | Operator and abuse contact | A (the ThinkersJournal entity and `admin@`), plus an abuse contact per service | [`NOTICE-AND-TERMS.md`](NOTICE-AND-TERMS.md) section 1; per-channel `abuse_contact` (PR #12) |
| 2 | Who can get a channel | Launch scope stays "people CireSnave names" (admin-issued) until the subdomain model is ruled. His plan: user-controlled services on subdomains under `thinkersjournal.net`; apex `thinkersjournal.com` only for administrator-surfaced services | [`DESIGN-SUBDOMAINS.md`](DESIGN-SUBDOMAINS.md) (options, awaiting a ruling) |
| 3 | Abuse handling | A: no content scanning; disable a channel at once; illegal-content reports go through the existing ThinkersJournal pipeline (board 119) | [`NOTICE-AND-TERMS.md`](NOTICE-AND-TERMS.md) section 2 |
| 4 | Privacy and retention | A, with the buffer kept only until an online service can receive it: 5 minutes (was 24 h) | [`NOTICE-AND-TERMS.md`](NOTICE-AND-TERMS.md) section 3; TTL change in PR #11 |
| 5 | Terms | A: one page | [`NOTICE-AND-TERMS.md`](NOTICE-AND-TERMS.md) section 4 |
| 6 | Cost ceiling | Not part of 447 (with board 445, the deploy) | `docs/DEPLOY.md` |

Open from 447: the subdomain model (a new decision, see `DESIGN-SUBDOMAINS.md`). The legal entity name is still the Community
lane's `[[LEGAL_ENTITY]]` placeholder (board 129).

-- lane: surfaceant
