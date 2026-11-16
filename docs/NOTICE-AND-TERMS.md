# SurfaceAnt: operator, abuse policy, privacy notice and terms

Draft text for the relay's public pages, written to CireSnave's rulings on board 447 (see [`ROW5-DECISIONS.md`](ROW5-DECISIONS.md)).
The legal entity is the placeholder `[[LEGAL_ENTITY]]` (board 129) until the Community lane names it. Not yet published: the
relay is deployed but not public: launch scope is people CireSnave names.

## 1. Who runs this, and who to tell about abuse

SurfaceAnt is operated by `[[LEGAL_ENTITY]]`, the same entity that operates ThinkersJournal. Contact and abuse reports:
`admin@thinkersjournal.com` (placeholder: the portfolio's `admin@` address; the Community lane confirms the exact one).

Each surfaced service also lists **its own abuse contact** (an email address or an `https://` page), set when the service is
set up. Report a problem with a particular service to that contact; report anything about SurfaceAnt as a whole, or a service
that does not answer, to the ThinkersJournal address above. If a service has no contact of its own, the ThinkersJournal
address is its contact.

## 2. Abuse policy

- We do **not** scan or read content passing through the relay.
- On a report, or on sight of abuse by rate, we **disable the channel at once**: sockets are closed and no further events are
  accepted. A reporter's note is kept with the decision.
- Reports of illegal content (child sexual abuse material and the like) follow ThinkersJournal's existing reporting pipeline
  (board 119), not a new one.
- Limits that make abuse expensive: 1 MiB per event, 10 requests per second per channel (burst 50, then `429`), at most
  1,000 buffered events and 64 MiB per channel.

## 3. Privacy notice

- **What we can see.** Traffic is encrypted to Cloudflare, which terminates TLS. The operator and Cloudflare can read the
  payloads of events in flight and buffered. There is **no end-to-end encryption** between sender and your machine. It cannot
  work for senders that sign plaintext, such as GitHub webhooks, and is a documented non-goal.
- **How long.** An event is buffered only until a service that is online can receive it. If your service is offline it is
  kept for **at most about 5 minutes**, then dropped. Events for an offline service are lost after that; the sender's own
  retry or redelivery is the recovery path.
- **Bodies are never logged.** We keep metadata logs (time, channel id, status, size) for **7 days**, then drop them.
  Cloudflare keeps its own request metadata under its own terms.
- Disabling a channel stops delivery at once; anything still buffered is dropped within 5 minutes.

## 4. Terms of use

One page, free, best effort:

1. SurfaceAnt is free and provided as is. **No uptime promise**; it can be slow, down or changed.
2. We may **suspend any channel or service at our discretion**, with or without notice.
3. Acceptable use: no malware delivery, no spam, no unlawful content, no traffic aimed at other people's systems.
4. **No student records, education records or children's personal data** go through the relay. SurfaceAnt is not built for
   FERPA or COPPA duties and there is no data-protection agreement.
5. You must be **13 or older**.
6. You are responsible for what is sent to, and done by, the program you connect to the relay.

-- lane: surfaceant
