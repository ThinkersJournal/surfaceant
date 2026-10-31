# SurfaceAnt design note (events mode)

Status: proposal for PM review. Options first, then a recommendation. No code.

## 1. What events mode must do

A sender POSTs `https://thinkersjournal.net/hooks/<channel-id>`. The relay passes the **method, selected headers and raw body
through untouched** (the user's program verifies the sender's signature; the relay never holds the sender's secret). A client
on the user's machine holds one outbound WebSocket to the channel. While the client is offline the relay buffers events up to
24 h; the client acks and the relay deletes. First customer: GitHub webhooks into `tools/gh_webhook_receiver.py`, which checks
`X-Hub-Signature-256` over the raw body and de-duplicates on the delivery id.

Cloudflare limits below were read from the Durable Objects pricing and limits pages on 2026-10-31 (Free plan) and **must be
re-read before deploy**:

- 100,000 DO requests/day; 13,000 GB-s/day duration; 5M SQLite rows read/day; 100k rows written/day; 5 GB stored.
- Incoming WebSocket messages bill 20:1; opening a WebSocket counts as one request.
- A WebSocket accepted with plain `accept()` bills duration for as long as it is connected; the **Hibernation API** avoids
  that once handlers return.
- A SQLite row or BLOB is at most 2 MB. A WebSocket message received by a DO is at most 32 MiB.
- Soft limit of about 1,000 requests/second per object.
- The free plan's per-request CPU limit for the Worker (PM read 10 ms) was *not* stated on the pages I fetched; treat as unverified.

Our load (about 1-3k events/day) fits the free plan. A public multi-user service would not; that is a cost question for
CireSnave, not a design one.

## 2. Options

### 2.1 Worker language

| | TypeScript | Rust -> WASM (`workers-rs`) |
|---|---|---|
| Fit | Native runtime; DO and Hibernation APIs are first-class and documented | Bindings lag the JS API; hibernation and SQL storage support must be checked per release |
| Tests | `vitest` + `@cloudflare/vitest-pool-workers` (Miniflare), `wrangler dev` | Same runtime, but a WASM build step in every test loop |
| Bundle / cold start | Small | Larger (WASM), more CPU at start - matters against a 10 ms free-plan budget |
| Shared code with the client | None (wire format specified in prose + test vectors) | Frame types could be one crate shared with `surfaceant-client` |
| Portfolio fit | Adds a Node toolchain and `npm` dependencies to a Rust shop | Stays in Rust |

**Recommendation: TypeScript** for the Worker, with a language-neutral wire spec (`docs/PROTOCOL.md`, later) and shared **test
vectors** (JSON files) that both the Worker tests and the Rust client tests consume. The sharing benefit of Rust/WASM is real
but small (the frame set is about six messages), while the CPU-budget and binding-lag risks are on the critical path of the
first customer. Revisit if the Worker grows logic beyond routing and storage. *Cost to the portfolio:* an npm lockfile and
Node CI job; CireSnave's "latest versions of dependencies" rule applies to it as well.

### 2.2 Connection model

- **A. One Durable Object per channel, WebSocket Hibernation API** (recommended). The DO id is derived from the channel id
  (`idFromName`), so inbound POSTs and the client's WebSocket reach the same object with no lookup table. Idle channels cost
  no duration.
- B. One global DO fanning out to channels: single hot object, 1,000 req/s soft cap, and one tenant's flood affects all.
  Rejected.
- C. Worker + Queues / KV buffer with the client polling: no WebSocket, so no push; KV is eventually consistent and cannot
  give ordered replay. Rejected for the "one outbound connection" mission.

### 2.3 Buffer

- **A. DO SQLite table** (recommended): `events(seq INTEGER PRIMARY KEY AUTOINCREMENT, delivery_id TEXT UNIQUE, received_at,
  headers BLOB, body BLOB, size)`. `seq` gives replay order; the unique `delivery_id` gives idempotency; body as BLOB keeps a
  1 MiB event inside the 2 MB row limit (base64 would not). Expiry via a DO **alarm** that deletes rows older than 24 h.
- B. DO key-value storage: same limits, no unique index; dedup would need a second key. Worse.

### 2.4 Authentication

Two separate secrets per channel, because the channel id is in a URL that senders (and their logs) see:

1. **Channel id** - 128-bit random, URL-safe, in the path. Knowing it lets you *post*, nothing else. (GitHub also signs, and
   the client's program verifies, so a forged post is rejected downstream; the relay's only job is to keep abuse cheap.)
2. **Connection token** - 256-bit random, sent by the client as `Authorization: Bearer` on the WebSocket upgrade (never in the
   URL, which gets logged). The relay stores only `SHA-256(token)` and compares in constant time
   (`crypto.subtle.timingSafeEqual`). Lets you *receive* and *ack*.

Provisioning in v1: an admin-only `POST /admin/channels` guarded by an admin secret (a Worker secret, never in git) that returns
channel id + token once. Self-service signup is mode-2 / public-service work and a CireSnave decision.

### 2.5 Delivery semantics

At-least-once, ordered per channel, idempotent by delivery id.

- The relay derives `delivery_id` from `X-GitHub-Delivery` when present, else from `SHA-256(body)` plus the channel's
  generic `X-Delivery-Id` / `Idempotency-Key` header if one exists, else a fresh UUID. A repeat of a stored id is answered `200`
  and **not** stored again. The client also remembers recent ids, and `gh_webhook_receiver.py` already de-duplicates, so
  duplicates are tolerated end to end.
- The relay answers the sender `202` once the event is **durably written** (not when the client acks), so senders do not retry
  storms against an offline client.
- On connect the client sends `hello{last_acked_seq}`; the relay replays every row with `seq > last_acked_seq` in order, then
  streams live events. Rows are deleted on `ack{seq}` (cumulative: acks everything up to `seq`).

## 3. Wire protocol (proposal)

WebSocket text frames, one JSON object each, `v` = 1.

| Direction | Frame | Fields |
|---|---|---|
| C -> R | `hello` | `v`, `last_acked_seq` |
| R -> C | `welcome` | `v`, `max_event_bytes`, `ttl_seconds`, `server_time` |
| R -> C | `event` | `seq`, `delivery_id`, `received_at`, `method`, `path_suffix`, `query`, `headers` (array of `[name, value]`), `body_b64` |
| C -> R | `ack` | `seq` (cumulative) |
| both | `ping` / `pong` | (also WebSocket-level auto-response so hibernation is not woken) |
| R -> C | `error` | `code`, `message`; relay then closes |

- `body_b64` is the raw body, base64 - JSON cannot carry arbitrary bytes. Byte-identity is a test: decode(`body_b64`) == bytes
  the sender posted, including for non-UTF-8 and trailing-newline bodies.
- Headers are forwarded as a list, not an object, to preserve order and duplicates. Hop-by-hop headers and `CF-*` / `X-Forwarded-*`
  added by Cloudflare are dropped; everything else (including `X-Hub-Signature-256`, `X-GitHub-Event`, `Content-Type`,
  `User-Agent`) is kept untouched.
- Base64 costs 33% on the WebSocket (max event 1 MiB -> about 1.4 MiB frame, well under the 32 MiB cap). Binary frames are a
  v2 optimisation, not worth complexity now.
- One live client per channel: a new connection **replaces** the old one (old gets `error{code:"replaced"}`), so a crashed client
  that left a half-open socket cannot block reconnection.

## 4. Limits and abuse controls

| Control | v1 value | Why |
|---|---|---|
| Max body | 1 MiB (`413` over; checked on `Content-Length` and while reading) | Fits the 2 MB row; matches the receiver |
| Buffer TTL | 24 h (alarm sweeps) | Brief; privacy |
| Buffer cap per channel | 1,000 events or 64 MiB, oldest-first drop with a counter surfaced in the next `welcome` | An offline client must not fill storage |
| Rate limit | per-channel token bucket in the DO, e.g. 10 req/s burst 50 -> `429` + `Retry-After` | DO is already the serialisation point, so no extra service |
| Disable | `POST /admin/channels/<id>/disable` flips a flag; disabled channels answer `410` and drop the socket | Required by the brief |
| Methods | POST only on `/hooks/<id>`; others `405` | |
| Logging | status + channel id hash + size only; never bodies or headers | Brief |
| Unknown channel | `404`, same response time as a disabled one is not required but body is empty | Do not confirm which ids exist |

Rate limiting per channel does not stop a flood of requests to *unknown* channel ids that each wake a DO. Mitigation: the Worker
checks a cheap existence signal first. Open question Q3.

## 5. Failure cases

- **Client offline:** events buffered, `202` to sender; replay on reconnect, in `seq` order, once.
- **Client slow:** relay sends one `event` at a time per unacked window (window of 16); if unacked beyond 30 s the relay stops
  sending and keeps buffering; no per-client unbounded memory (state lives in SQLite).
- **Duplicate delivery from sender:** same `delivery_id` -> `200`, not re-stored. **Duplicate to client** (reconnect before ack):
  allowed; client de-duplicates by `delivery_id`/`seq`.
- **Oversize:** `413` before buffering. **Wrong or missing token:** `401` on upgrade, no body detail. **Disabled:** `410`.
- **DO eviction / deploy mid-stream:** all state is in SQLite; clients reconnect with `last_acked_seq`.
- **Daily free-plan quota exhausted:** Cloudflare fails requests; senders see 5xx and GitHub retries manually. Surfaced as an
  operations risk; monitoring is a deploy-row item.
- **Clock skew:** TTL uses only the relay's clock.

## 6. How mode 2 (surface) reuses this

Same DO-per-name + Hibernation + bearer-token + hello/welcome skeleton. Differences, which is why it is a separate design:
requests are *interactive* (need a response routed back: request ids, streaming, backpressure), nothing is buffered, hostnames
are per-tunnel subdomains (`<name>.thinkersjournal.net`, first level only for Universal SSL), and public user content needs a
Public Suffix List entry first. Frame set would add `req`/`res`/`body-chunk`/`cancel`; `hello`, `ack`, `ping`, `error` carry over,
so the frame `v` and a `kind` (`events` | `surface`) are reserved now.

## 7. Recommendation

Worker in TypeScript; one SQLite-backed, hibernating Durable Object per channel; the frames in section 3; at-least-once ordered
delivery with delivery-id idempotency; the limits in section 4. Language-neutral test vectors shared by the Worker and the Rust
client.

## 8. Open questions for the PM (not CireSnave)

- **Q1.** TypeScript Worker acceptable given the Rust-shop norm (adds Node + npm to CI)?
- **Q2.** Is the admin-provisioned channel model enough for v1, with self-service deferred to the surface/public design?
- **Q3.** Unknown-id flood: accept the cost for now (own use), or add a stateless HMAC on the channel id so the Worker rejects
  forged ids without touching a DO? (HMAC makes the channel id self-validating: `id = rand || MAC(rand)`, no lookup needed.)
- **Q4.** Buffer caps (1,000 events / 64 MiB) and rate limit numbers: fine for our own traffic?

Decision items for CireSnave (abuse policy, retention statement, terms, what happens at the free-plan quota) are collected
separately via the PM and are not decided here.
