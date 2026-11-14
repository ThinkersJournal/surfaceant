// SPDX-License-Identifier: MIT OR Apache-2.0

//! SurfaceAnt relay Worker. Skeleton: verification probes only; the channel Durable Object comes next.

use worker::*;

/// 16 random bytes from the Workers runtime (`crypto.getRandomValues` through `getrandom`), as 32 hex digits.
fn random_id() -> std::result::Result<String, getrandom::Error> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    match req.url()?.path() {
        "/check/random" => Response::ok(random_id().map_err(|e| Error::from(e.to_string()))?),
        _ => Ok(Response::empty()?.with_status(404)),
    }
}
