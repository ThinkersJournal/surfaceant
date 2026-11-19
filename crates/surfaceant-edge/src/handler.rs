//! Slice 2: a hickory `RequestHandler` that answers from [`Synth`] instead of a zone file.
//! The synthesis decisions live in `synth.rs`; this file only maps them onto DNS records and
//! response codes, sets AA, and caps the EDNS payload at 1232 bytes.

use std::{
    str::FromStr,
    sync::{Arc, RwLock},
};

use hickory_proto::{
    op::{Edns, Header, HeaderCounts, MessageType, Metadata, OpCode, ResponseCode},
    rr::{
        Name, RData, Record, RecordType,
        rdata::{A, AAAA, CAA, NS, SOA, TXT},
    },
};
use hickory_server::{
    net::xfer::Protocol,
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::MessageResponseBuilder,
};

use crate::{
    rate::RateLimiter,
    synth::{Answer, Data, QType, Synth},
};

/// Largest UDP payload we advertise and honour (DESIGN-VPS-EDGE 1.1: truncate to TCP).
pub const MAX_UDP: u16 = 1232;

/// The live synthesis, swapped whole on a registry reload; a query takes one `Arc` and
/// answers from that snapshot, so a reload never changes an answer half way.
pub type SynthCell = Arc<RwLock<Arc<Synth>>>;

/// Make a [`SynthCell`].
pub fn cell(s: Synth) -> SynthCell {
    Arc::new(RwLock::new(Arc::new(s)))
}

/// Answers one zone from a [`Synth`].
pub struct EdgeHandler {
    pub synth: SynthCell,
    pub ttl: u32,
    pub soa: SOA,
    /// Per-source limit for UDP (spoofable); over-limit queries get no reply at all.
    pub limiter: Option<RateLimiter>,
    /// Origin of the limiter's clock.
    pub started: std::time::Instant,
}

fn qtype(t: RecordType) -> QType {
    match t {
        RecordType::A => QType::A,
        RecordType::AAAA => QType::Aaaa,
        RecordType::NS => QType::Ns,
        RecordType::SOA => QType::Soa,
        RecordType::TXT => QType::Txt,
        RecordType::CAA => QType::Caa,
        RecordType::ANY => QType::Any,
        _ => QType::Other,
    }
}

impl EdgeHandler {
    fn record(&self, owner: &Name, d: &Data) -> Option<Record> {
        let rdata = match d {
            Data::A(a) => RData::A(A(*a)),
            Data::Aaaa(a) => RData::AAAA(AAAA(*a)),
            Data::Ns(n) => RData::NS(NS(Name::from_ascii(format!("{n}.")).ok()?)),
            Data::Soa => RData::SOA(self.soa.clone()),
            Data::Txt(t) => RData::TXT(TXT::new(vec![t.clone()])),
            Data::Caa(issuer) => RData::CAA(CAA::new_issue(
                false,
                Some(Name::from_str(issuer).ok()?),
                vec![],
            )),
        };
        Some(Record::from_rdata(owner.clone(), self.ttl, rdata))
    }
}

#[async_trait::async_trait]
impl RequestHandler for EdgeHandler {
    async fn handle_request<R: ResponseHandler, T: hickory_server::net::runtime::Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        if let Some(l) = &self.limiter
            && request.protocol() == Protocol::Udp
            && !l.allow(request.src().ip(), self.started.elapsed())
        {
            // Dropped, not refused: a refusal is a reply a spoofed source would receive.
            let mut metadata = Metadata::response_from_request(&request.metadata);
            metadata.response_code = ResponseCode::Refused;
            return ResponseInfo::from(Header {
                metadata,
                counts: HeaderCounts::default(),
            });
        }
        let mut edns = None;
        if let Some(req) = request.edns.as_ref() {
            let mut e = Edns::new();
            e.set_max_payload(req.max_payload().clamp(512, MAX_UDP));
            e.set_version(0);
            edns = Some(e);
        }
        let builder = MessageResponseBuilder::new(&request.queries, edns.as_ref());

        let bad = |code| {
            MessageResponseBuilder::new(&request.queries, edns.as_ref())
                .error_msg(&request.metadata, code)
        };
        if request.metadata.message_type != MessageType::Query {
            return send(&mut response_handle, bad(ResponseCode::FormErr)).await;
        }
        if request.metadata.op_code != OpCode::Query {
            return send(&mut response_handle, bad(ResponseCode::NotImp)).await;
        }
        let Ok(info) = request.request_info() else {
            return send(&mut response_handle, bad(ResponseCode::FormErr)).await;
        };
        let q = info.query;
        let qname = q.name().to_string();
        let owner: Name = q.name().into();

        let snapshot: Arc<Synth> = match self.synth.read() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };
        let mut meta = Metadata::response_from_request(&request.metadata);
        meta.authoritative = true;
        meta.recursion_available = false;

        let soa_rec = self.record(
            &Name::from_ascii(format!("{}.", snapshot.apex)).unwrap(),
            &Data::Soa,
        );
        match snapshot.answer(&qname, qtype(q.query_type())) {
            Answer::Refused => {
                meta.authoritative = false;
                meta.response_code = ResponseCode::Refused;
                send(&mut response_handle, builder.build_no_records(meta)).await
            }
            Answer::Records(ds) => {
                let recs: Vec<Record> = ds.iter().filter_map(|d| self.record(&owner, d)).collect();
                meta.response_code = ResponseCode::NoError;
                let none: Vec<Record> = vec![];
                send(
                    &mut response_handle,
                    builder.build(meta, recs.iter(), none.iter(), none.iter(), none.iter()),
                )
                .await
            }
            a @ (Answer::NoData | Answer::NxDomain) => {
                meta.response_code = if a == Answer::NxDomain {
                    ResponseCode::NXDomain
                } else {
                    ResponseCode::NoError
                };
                let none: Vec<Record> = vec![];
                let soa: Vec<Record> = soa_rec.into_iter().collect();
                send(
                    &mut response_handle,
                    builder.build(meta, none.iter(), none.iter(), soa.iter(), none.iter()),
                )
                .await
            }
        }
    }
}

async fn send<'a, R: ResponseHandler>(
    h: &mut R,
    resp: hickory_server::zone_handler::MessageResponse<
        '_,
        'a,
        impl Iterator<Item = &'a Record> + Send + 'a,
        impl Iterator<Item = &'a Record> + Send + 'a,
        impl Iterator<Item = &'a Record> + Send + 'a,
        impl Iterator<Item = &'a Record> + Send + 'a,
    >,
) -> ResponseInfo {
    match h.send_response(resp).await {
        Ok(i) => i,
        Err(_) => {
            let mut metadata = Metadata::new(0, MessageType::Response, OpCode::Query);
            metadata.response_code = ResponseCode::ServFail;
            ResponseInfo::from(Header {
                metadata,
                counts: HeaderCounts::default(),
            })
        }
    }
}
