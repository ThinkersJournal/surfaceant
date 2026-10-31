use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let mut seg = url.path_segments().into_iter().flatten();
    // /check/* (second-spike probes) all share one object; /hooks/<id> and /connect/<id> use <id>.
    let id = match seg.next() {
        Some("check") => Some("check"),
        _ => seg.next().filter(|s| !s.is_empty()),
    };
    let Some(id) = id else {
        return Ok(Response::empty()?.with_status(404));
    };
    env.durable_object("CHANNEL")?
        .id_from_name(id)?
        .get_stub()?
        .fetch_with_request(req)
        .await
}

#[durable_object]
pub struct Channel {
    state: State,
}

#[derive(Serialize)]
struct Frame {
    seq: i64,
    headers: Vec<(String, String)>,
    body_b64: String,
}

#[derive(Deserialize)]
struct Ack {
    ack: i64,
}

fn frame(row: Vec<SqlStorageValue>) -> Result<String> {
    let (SqlStorageValue::Integer(seq), SqlStorageValue::String(h), SqlStorageValue::Blob(b)) =
        (&row[0], &row[1], &row[2])
    else {
        return Err("unexpected row shape".into());
    };
    let f = Frame {
        seq: *seq,
        headers: serde_json::from_str(h)?,
        body_b64: STANDARD.encode(b),
    };
    Ok(serde_json::to_string(&f)?)
}

impl DurableObject for Channel {
    fn new(state: State, _env: Env) -> Self {
        let q = "CREATE TABLE IF NOT EXISTS ev (seq INTEGER PRIMARY KEY AUTOINCREMENT, headers TEXT NOT NULL, body BLOB NOT NULL)";
        state.storage().sql().exec(q, None).expect("create table");
        Self { state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        if url.path().starts_with("/check/") {
            return self.check(&mut req, &url).await;
        }
        let sql = self.state.storage().sql();
        if req.headers().get("Upgrade")?.as_deref() == Some("websocket") {
            let pair = WebSocketPair::new()?;
            self.state.accept_web_socket(&pair.server); // hibernation API
            for r in sql
                .exec("SELECT seq, headers, body FROM ev ORDER BY seq", None)?
                .raw()
            {
                pair.server.send_with_str(frame(r?)?)?; // replay
            }
            return Response::from_websocket(pair.client);
        }
        let body = req.bytes().await?;
        let headers: Vec<(String, String)> = req.headers().entries().collect();
        let bind = vec![
            SqlStorageValue::String(serde_json::to_string(&headers)?),
            SqlStorageValue::Blob(body),
        ];
        let row = sql
            .exec(
                "INSERT INTO ev (headers, body) VALUES (?, ?) RETURNING seq, headers, body",
                bind,
            )?
            .raw()
            .next()
            .ok_or("no row")??;
        let text = frame(row)?;
        for ws in self.state.get_websockets() {
            ws.send_with_str(&text)?;
        }
        Ok(Response::empty()?.with_status(202))
    }

    async fn alarm(&self) -> Result<Response> {
        let sql = self.state.storage().sql();
        sql.exec("CREATE TABLE IF NOT EXISTS fired (n INTEGER)", None)?;
        sql.exec("INSERT INTO fired VALUES (1)", None)?;
        Response::empty()
    }

    async fn websocket_message(
        &self,
        _ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        if let WebSocketIncomingMessage::String(s) = message {
            if let Ok(a) = serde_json::from_str::<Ack>(&s) {
                self.state.storage().sql().exec(
                    "DELETE FROM ev WHERE seq <= ?",
                    vec![SqlStorageValue::Integer(a.ack)],
                )?;
            }
        }
        Ok(())
    }

    async fn websocket_close(
        &self,
        _ws: WebSocket,
        _code: usize,
        _reason: String,
        _clean: bool,
    ) -> Result<()> {
        Ok(())
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

impl Channel {
    /// Second-spike checks: HMAC-SHA-256 verify, alarms, capped streaming body read.
    async fn check(&self, req: &mut Request, url: &Url) -> Result<Response> {
        let q: std::collections::HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        match url.path() {
            "/check/hmac" => {
                let mut mac = Hmac::<Sha256>::new_from_slice(q["key"].as_bytes())
                    .map_err(|e| Error::from(e.to_string()))?;
                mac.update(q["msg"].as_bytes());
                let expected = unhex(&q["mac"]).unwrap_or_default();
                // verify_slice compares in constant time (subtle::ConstantTimeEq).
                let ok = mac.verify_slice(&expected).is_ok();
                Ok(Response::empty()?.with_status(if ok { 200 } else { 401 }))
            }
            "/check/alarm-set" => {
                self.state
                    .storage()
                    .set_alarm(std::time::Duration::from_millis(100))
                    .await?;
                Response::empty()
            }
            "/check/alarm-fired" => {
                let sql = self.state.storage().sql();
                sql.exec("CREATE TABLE IF NOT EXISTS fired (n INTEGER)", None)?;
                let n = sql
                    .exec("SELECT COUNT(*) FROM fired", None)?
                    .raw()
                    .next()
                    .ok_or("no row")??;
                let SqlStorageValue::Integer(n) = n[0] else {
                    return Err("bad count".into());
                };
                Response::ok(n.to_string())
            }
            "/check/capped" => {
                let max: usize = q["max"].parse().map_err(|_| Error::from("bad max"))?;
                let mut total = 0usize;
                let mut stream = req.stream()?;
                while let Some(chunk) = stream.next().await {
                    total += chunk?.len();
                    if total > max {
                        return Ok(Response::empty()?.with_status(413)); // stop reading; the rest is never buffered
                    }
                }
                Response::ok(total.to_string())
            }
            _ => Ok(Response::empty()?.with_status(404)),
        }
    }
}
