// SPDX-License-Identifier: MIT OR Apache-2.0

//! SurfaceAnt relay Worker (events mode): the router and one hibernating Durable Object per channel.
//! See `docs/DESIGN.md`. Pure logic lives in [`logic`] and is unit-tested on the host.

pub mod logic;

use futures_util::StreamExt;
use logic::{Limits, TokenBucket};
use serde::{Deserialize, Serialize};
use surfaceant_proto::{Event, Frame, PROTOCOL_VERSION};
use worker::*;

const PING: &str = r#"{"type":"ping"}"#;
const PONG: &str = r#"{"type":"pong"}"#;

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| Error::from(e.to_string()))?;
    Ok(b)
}

fn status(code: u16) -> Result<Response> {
    Ok(Response::empty()?.with_status(code))
}

fn env_num<T: std::str::FromStr>(env: &Env, name: &str) -> Option<T> {
    env.var(name).ok()?.to_string().trim().parse().ok()
}

fn limits(env: &Env) -> Limits {
    let d = Limits::default();
    Limits {
        max_body_bytes: env_num(env, "MAX_BODY_BYTES").unwrap_or(d.max_body_bytes),
        buffer_ttl_seconds: env_num(env, "BUFFER_TTL_SECONDS").unwrap_or(d.buffer_ttl_seconds),
        buffer_max_events: env_num(env, "BUFFER_MAX_EVENTS").unwrap_or(d.buffer_max_events),
        buffer_max_bytes: env_num(env, "BUFFER_MAX_BYTES").unwrap_or(d.buffer_max_bytes),
        rate_per_sec: env_num(env, "RATE_PER_SEC").unwrap_or(d.rate_per_sec),
        rate_burst: env_num(env, "RATE_BURST").unwrap_or(d.rate_burst),
        window: env_num(env, "WINDOW").unwrap_or(d.window),
    }
}

fn iso(ms: i64) -> String {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64))
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

fn stub(env: &Env, channel_id: &str) -> Result<Stub> {
    env.durable_object("CHANNEL")?
        .id_from_name(channel_id)?
        .get_stub()
}

fn secret(env: &Env, name: &str) -> Result<String> {
    Ok(env.secret(name)?.to_string())
}

/// `Authorization: Bearer <ADMIN_SECRET>` check; a missing secret binding refuses everyone.
fn admin_ok(req: &Request, env: &Env) -> Result<bool> {
    let Ok(want) = secret(env, "ADMIN_SECRET") else {
        return Ok(false);
    };
    let header = req.headers().get("Authorization")?;
    Ok(!want.is_empty()
        && logic::bearer(header.as_deref()).is_some_and(|got| {
            logic::secret_eq(&logic::hash_token(got), &logic::hash_token(&want))
        }))
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let segs: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    let key = secret(&env, "CHANNEL_KEY").unwrap_or_default();
    let method = req.method();
    match (segs.as_slice(), method) {
        (["hooks", id, ..], Method::Post) => {
            if key.is_empty() || !logic::channel_id_is_valid(key.as_bytes(), id) {
                return status(404); // unknown ids never reach a Durable Object
            }
            stub(&env, id)?.fetch_with_request(req).await
        }
        (["hooks", ..], _) => status(405),
        (["connect", id], Method::Get) => {
            if key.is_empty() || !logic::channel_id_is_valid(key.as_bytes(), id) {
                return status(404);
            }
            stub(&env, id)?.fetch_with_request(req).await
        }
        (["admin", "channels"], Method::Post) => {
            if !admin_ok(&req, &env)? {
                return status(401);
            }
            if key.is_empty() {
                return status(500);
            }
            let id = logic::make_channel_id(key.as_bytes(), random_bytes()?);
            let token = logic::hex(&random_bytes::<32>()?);
            // Optional JSON body {"abuse_contact": "..."}: this service's own abuse contact (board 447).
            let raw = req.text().await.unwrap_or_default();
            let abuse_contact = if raw.trim().is_empty() {
                None
            } else {
                let parsed: serde_json::Value = match serde_json::from_str(&raw) {
                    Ok(v) => v,
                    Err(_) => return status(400),
                };
                match parsed.get("abuse_contact") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s)) if logic::abuse_contact_is_valid(s) => {
                        Some(s.trim().to_string())
                    }
                    Some(_) => return status(400),
                }
            };
            let mut init = RequestInit::new();
            init.with_method(Method::Post).with_body(Some(
                serde_json::json!({
                    "token_hash": logic::hash_token(&token),
                    "abuse_contact": abuse_contact,
                })
                .to_string()
                .into(),
            ));
            let r = Request::new_with_init("https://channel/init", &init)?;
            let resp = stub(&env, &id)?.fetch_with_request(r).await?;
            if resp.status_code() != 201 {
                return status(500);
            }
            Ok(
                Response::from_json(&serde_json::json!({"channel_id": id, "token": token, "abuse_contact": abuse_contact}))?
                    .with_status(201),
            )
        }
        (["admin", "channels", id], Method::Get) => {
            if !admin_ok(&req, &env)? {
                return status(401);
            }
            if key.is_empty() || !logic::channel_id_is_valid(key.as_bytes(), id) {
                return status(404);
            }
            let r = Request::new("https://channel/info", Method::Get)?;
            stub(&env, id)?.fetch_with_request(r).await
        }
        (["admin", "channels", id, "disable"], Method::Post) => {
            if !admin_ok(&req, &env)? {
                return status(401);
            }
            if key.is_empty() || !logic::channel_id_is_valid(key.as_bytes(), id) {
                return status(404);
            }
            let mut init = RequestInit::new();
            init.with_method(Method::Post);
            let r = Request::new_with_init("https://channel/disable", &init)?;
            stub(&env, id)?.fetch_with_request(r).await
        }
        _ => status(404),
    }
}

#[derive(Serialize, Deserialize)]
struct Attachment {
    hello: bool,
}

/// One channel: its token hash, buffered events, connected client and rate limiter, in a SQLite-backed Durable Object.
#[durable_object]
pub struct Channel {
    state: State,
    env: Env,
    bucket: std::cell::RefCell<Option<TokenBucket>>,
}

type Row = Vec<SqlStorageValue>;

impl Channel {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn rows(&self, q: &str, binds: Vec<SqlStorageValue>) -> Result<Vec<Row>> {
        self.sql().exec(q, binds)?.raw().collect()
    }

    fn scalar(&self, q: &str, binds: Vec<SqlStorageValue>) -> Result<i64> {
        match self.rows(q, binds)?.first().and_then(|r| r.first()) {
            Some(SqlStorageValue::Integer(n)) => Ok(*n),
            Some(SqlStorageValue::Null) | None => Ok(0),
            other => Err(format!("expected an integer, got {other:?}").into()),
        }
    }

    fn meta_get(&self, k: &str) -> Result<Option<String>> {
        let rows = self.rows("SELECT v FROM meta WHERE k = ?", vec![k.into()])?;
        Ok(match rows.first().and_then(|r| r.first()) {
            Some(SqlStorageValue::String(s)) => Some(s.clone()),
            _ => None,
        })
    }

    fn meta_set(&self, k: &str, v: &str) -> Result<()> {
        self.sql().exec(
            "INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            vec![k.into(), v.into()],
        )?;
        Ok(())
    }

    fn meta_i64(&self, k: &str) -> Result<i64> {
        Ok(self.meta_get(k)?.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    fn row_to_event(row: &Row) -> Result<Event> {
        let [
            SqlStorageValue::Integer(seq),
            SqlStorageValue::String(delivery_id),
            SqlStorageValue::Integer(received_ms),
            SqlStorageValue::String(method),
            SqlStorageValue::String(path_suffix),
            SqlStorageValue::String(query),
            SqlStorageValue::String(headers),
            SqlStorageValue::Blob(body),
        ] = row.as_slice()
        else {
            return Err("unexpected event row shape".into());
        };
        Ok(Event {
            seq: *seq,
            delivery_id: delivery_id.clone(),
            received_at: iso(*received_ms),
            method: method.clone(),
            path_suffix: path_suffix.clone(),
            query: query.clone(),
            headers: serde_json::from_str(headers)?,
            body_b64: String::new(),
        }
        .with_body(body))
    }

    fn live_sockets(&self) -> Vec<WebSocket> {
        self.state
            .get_websockets()
            .into_iter()
            .filter(|w| matches!(w.deserialize_attachment::<Attachment>(), Ok(Some(a)) if a.hello))
            .collect()
    }

    /// Send the next events to the connected client, keeping at most `window` unacknowledged.
    fn pump(&self) -> Result<()> {
        let sockets = self.live_sockets();
        if sockets.is_empty() {
            return Ok(());
        }
        let window = limits(&self.env).window;
        let acked = self.meta_i64("acked_seq")?;
        let sent = self.meta_i64("sent_upto")?.max(acked);
        let in_flight = self.scalar(
            "SELECT COUNT(*) FROM events WHERE seq > ? AND seq <= ?",
            vec![acked.into(), sent.into()],
        )?;
        let room = window - in_flight;
        if room <= 0 {
            return Ok(());
        }
        let rows = self.rows(
            "SELECT seq, delivery_id, received_ms, method, path_suffix, query, headers, body \
             FROM events WHERE seq > ? ORDER BY seq LIMIT ?",
            vec![sent.into(), room.into()],
        )?;
        let mut last = sent;
        for row in &rows {
            let ev = Self::row_to_event(row)?;
            last = ev.seq;
            let text = Frame::Event(ev).to_text();
            for ws in &sockets {
                ws.send_with_str(&text)?;
            }
        }
        self.meta_set("sent_upto", &last.to_string())
    }

    fn error_frame(ws: &WebSocket, code: &str, message: &str) {
        let f = Frame::Error {
            code: code.into(),
            message: message.into(),
        };
        let _ = ws.send_with_str(f.to_text());
    }

    async fn hook(&self, mut req: Request) -> Result<Response> {
        if self.meta_get("token_hash")?.is_none() {
            return status(404);
        }
        if self.meta_get("disabled")?.is_some() {
            return status(410);
        }
        let lim = limits(&self.env);
        let now = now_ms();
        {
            let mut b = self.bucket.borrow_mut();
            let bucket = b.get_or_insert_with(|| TokenBucket::new(&lim, now));
            if let Err(wait) = bucket.take(&lim, now) {
                let mut r = status(429)?;
                r.headers_mut().set("Retry-After", &wait.to_string())?;
                return Ok(r);
            }
        }
        let declared: Option<usize> = req
            .headers()
            .get("Content-Length")?
            .and_then(|v| v.trim().parse().ok());
        if declared.is_some_and(|n| n > lim.max_body_bytes) {
            return status(413);
        }
        let mut body: Vec<u8> = Vec::new();
        let mut stream = req.stream()?;
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk?);
            if body.len() > lim.max_body_bytes {
                return status(413); // the rest of the body is never buffered
            }
        }
        let url = req.url()?;
        let path = url.path();
        let suffix = path
            .strip_prefix("/hooks/")
            .and_then(|r| r.split_once('/'))
            .map(|(_, rest)| format!("/{rest}"))
            .unwrap_or_default();
        let headers: Vec<(String, String)> = req
            .headers()
            .entries()
            .filter(|(n, _)| logic::forward_header(n))
            .collect();
        let delivery_id = match logic::delivery_id_from(&headers) {
            Some(d) => d,
            None => logic::hex(&random_bytes::<16>()?),
        };
        if self.scalar(
            "SELECT COUNT(*) FROM events WHERE delivery_id = ?",
            vec![delivery_id.clone().into()],
        )? > 0
        {
            return status(200); // already stored: answered, not stored twice
        }
        self.sql().exec(
            "INSERT INTO events (delivery_id, received_ms, method, path_suffix, query, headers, body, size) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                delivery_id.into(),
                (now as i64).into(),
                req.method().to_string().into(),
                suffix.into(),
                url.query().unwrap_or("").into(),
                serde_json::to_string(&headers)?.into(),
                SqlStorageValue::Blob(body.clone()),
                (body.len() as i64).into(),
            ],
        )?;
        self.enforce_caps(&lim)?;
        if self.state.storage().get_alarm().await?.is_none() {
            self.state
                .storage()
                .set_alarm(std::time::Duration::from_secs(lim.buffer_ttl_seconds))
                .await?;
        }
        self.pump()?;
        status(202) // after the durable write, not after the client acks
    }

    /// Drop the oldest events past the count and byte caps, and remember how many.
    fn enforce_caps(&self, lim: &Limits) -> Result<()> {
        let mut dropped = 0;
        let count = self.scalar("SELECT COUNT(*) FROM events", vec![])?;
        if count > lim.buffer_max_events {
            let n = count - lim.buffer_max_events;
            self.sql().exec(
                "DELETE FROM events WHERE seq IN (SELECT seq FROM events ORDER BY seq LIMIT ?)",
                vec![n.into()],
            )?;
            dropped += n;
        }
        while self.scalar("SELECT COALESCE(SUM(size), 0) FROM events", vec![])?
            > lim.buffer_max_bytes
            && self.scalar("SELECT COUNT(*) FROM events", vec![])? > 1
        {
            self.sql().exec(
                "DELETE FROM events WHERE seq = (SELECT MIN(seq) FROM events)",
                vec![],
            )?;
            dropped += 1;
        }
        if dropped > 0 {
            let total = self.meta_i64("dropped")? + dropped;
            self.meta_set("dropped", &total.to_string())?;
        }
        Ok(())
    }

    fn accept_client(&self, req: &Request) -> Result<Response> {
        let Some(want) = self.meta_get("token_hash")? else {
            return status(404);
        };
        if self.meta_get("disabled")?.is_some() {
            return status(410);
        }
        let auth = req.headers().get("Authorization")?;
        let ok = logic::bearer(auth.as_deref())
            .is_some_and(|t| logic::secret_eq(&logic::hash_token(t), &want));
        if !ok {
            return status(401);
        }
        if req
            .headers()
            .get("Upgrade")?
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
            != Some("websocket")
        {
            return status(426);
        }
        let pair = WebSocketPair::new()?;
        for old in self.state.get_websockets() {
            Self::error_frame(
                &old,
                "replaced",
                "another connection took over this channel",
            );
            let _ = old.close(Some(4000), Some("replaced"));
        }
        pair.server
            .serialize_attachment(Attachment { hello: false })?;
        self.state.accept_web_socket(&pair.server); // hibernation API
        Response::from_websocket(pair.client)
    }

    fn handle_hello(&self, ws: &WebSocket, v: u32, last_acked: i64) -> Result<()> {
        if v != PROTOCOL_VERSION {
            Self::error_frame(
                ws,
                "unsupported_version",
                "this relay speaks protocol version 1",
            );
            let _ = ws.close(Some(4001), Some("unsupported_version"));
            return Ok(());
        }
        let last_acked = last_acked.max(0);
        self.sql()
            .exec("DELETE FROM events WHERE seq <= ?", vec![last_acked.into()])?;
        self.meta_set("acked_seq", &last_acked.to_string())?;
        self.meta_set("sent_upto", &last_acked.to_string())?;
        let lim = limits(&self.env);
        let welcome = Frame::Welcome {
            v: PROTOCOL_VERSION,
            max_event_bytes: lim.max_body_bytes as u64,
            ttl_seconds: lim.buffer_ttl_seconds,
            server_time: iso(now_ms() as i64),
            dropped_events: self.meta_i64("dropped")? as u64,
        };
        self.meta_set("dropped", "0")?;
        ws.send_with_str(welcome.to_text())?;
        ws.serialize_attachment(Attachment { hello: true })?;
        self.pump()
    }

    fn handle_ack(&self, seq: i64) -> Result<()> {
        let sent = self.meta_i64("sent_upto")?;
        let seq = seq.min(sent); // never delete what was not sent
        if seq <= self.meta_i64("acked_seq")? {
            return Ok(());
        }
        self.sql()
            .exec("DELETE FROM events WHERE seq <= ?", vec![seq.into()])?;
        self.meta_set("acked_seq", &seq.to_string())?;
        self.pump()
    }
}

impl DurableObject for Channel {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        for q in [
            "CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY AUTOINCREMENT, delivery_id TEXT NOT NULL UNIQUE, \
             received_ms INTEGER NOT NULL, method TEXT NOT NULL, path_suffix TEXT NOT NULL, query TEXT NOT NULL, \
             headers TEXT NOT NULL, body BLOB NOT NULL, size INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
        ] {
            sql.exec(q, None).expect("create schema");
        }
        if let Ok(pair) = worker_sys::WebSocketRequestResponsePair::new(PING, PONG) {
            state.set_websocket_auto_response(&pair); // keep hibernation asleep for pings
        }
        Self {
            state,
            env,
            bucket: std::cell::RefCell::new(None),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        let path = url.path().to_string();
        if path == "/init" {
            if self.meta_get("token_hash")?.is_some() {
                return status(409);
            }
            let init: serde_json::Value = serde_json::from_str(&req.text().await?)?;
            let hash = init["token_hash"].as_str().unwrap_or_default();
            if hash.is_empty() {
                return status(400);
            }
            self.meta_set("token_hash", hash.trim())?;
            if let Some(c) = init["abuse_contact"].as_str() {
                self.meta_set("abuse_contact", c)?;
            }
            return status(201);
        }
        if path == "/info" {
            if self.meta_get("token_hash")?.is_none() {
                return status(404);
            }
            return Response::from_json(&serde_json::json!({
                "abuse_contact": self.meta_get("abuse_contact")?,
                "disabled": self.meta_get("disabled")?.is_some(),
            }));
        }
        if path == "/disable" {
            if self.meta_get("token_hash")?.is_none() {
                return status(404);
            }
            self.meta_set("disabled", "1")?;
            for ws in self.state.get_websockets() {
                Self::error_frame(&ws, "disabled", "this channel was disabled");
                let _ = ws.close(Some(4002), Some("disabled"));
            }
            return status(204);
        }
        if path.starts_with("/hooks/") {
            return self.hook(req).await;
        }
        if path.starts_with("/connect/") {
            return self.accept_client(&req);
        }
        status(404)
    }

    async fn alarm(&self) -> Result<Response> {
        let lim = limits(&self.env);
        let ttl_ms = (lim.buffer_ttl_seconds * 1000) as i64;
        let cutoff = now_ms() as i64 - ttl_ms;
        self.sql().exec(
            "DELETE FROM events WHERE received_ms < ?",
            vec![cutoff.into()],
        )?;
        let oldest = self.scalar("SELECT COALESCE(MIN(received_ms), 0) FROM events", vec![])?;
        if oldest > 0 {
            let at = (oldest + ttl_ms).max(now_ms() as i64 + 1000);
            self.state
                .storage()
                .set_alarm(std::time::Duration::from_millis(
                    (at - now_ms() as i64) as u64,
                ))
                .await?;
        }
        Response::empty()
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let WebSocketIncomingMessage::String(text) = message else {
            Self::error_frame(&ws, "protocol", "text frames only");
            let _ = ws.close(Some(1003), Some("text frames only"));
            return Ok(());
        };
        let helloed = matches!(ws.deserialize_attachment::<Attachment>(), Ok(Some(a)) if a.hello);
        match Frame::from_text(&text) {
            Ok(Frame::Hello { v, last_acked_seq }) if !helloed => {
                self.handle_hello(&ws, v, last_acked_seq)
            }
            Ok(Frame::Ack { seq }) if helloed => self.handle_ack(seq),
            Ok(Frame::Ping) => ws.send_with_str(PONG),
            Ok(Frame::Pong) => Ok(()),
            _ => {
                Self::error_frame(&ws, "protocol", "unexpected or malformed frame");
                let _ = ws.close(Some(1002), Some("protocol"));
                Ok(())
            }
        }
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        code: usize,
        _reason: String,
        _clean: bool,
    ) -> Result<()> {
        let _ = ws.close(Some(code.clamp(1000, 4999) as u16), Some("bye"));
        Ok(())
    }
}
