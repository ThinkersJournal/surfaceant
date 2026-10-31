use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let id = url
        .path_segments()
        .and_then(|mut s| s.nth(1))
        .filter(|s| !s.is_empty());
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
