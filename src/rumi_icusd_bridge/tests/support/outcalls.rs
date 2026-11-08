//! PocketIC HTTPS-outcall pump and a provider router in front of a local anvil.
//!
//! # How outcalls work under PocketIC
//!
//! PocketIC never makes real network requests unless the instance is in
//! auto-progress mode. Otherwise every canister HTTPS outcall parks until the
//! test driver fetches it with `PocketIc::get_canister_http()` and answers it
//! with `PocketIc::mock_canister_http_response(MockCanisterHttpResponse { .. })`.
//! This module loops `tick -> get_canister_http -> route -> mock response` until
//! the thing under test is done.
//!
//! # REAL versus MOCKED evidence (read this before writing an assertion)
//!
//! * A request routed with [`FaultAction::Pass`] is forwarded unchanged to the
//!   local anvil node and anvil's response body comes back with HTTP 200. That is
//!   REAL EVM execution: any transaction really runs on anvil, any log really
//!   exists on the anvil chain.
//! * Every other [`FaultAction`] (`ReplaceBody`, `HttpStatus`, `Reject`,
//!   `DelayOnce`) fabricates or withholds the provider's answer. A
//!   `ReplaceBody` response is MOCKED external-chain evidence: it did not come
//!   from the chain. Scenario tests must say so in their names/comments and must
//!   not treat it as proof that anything happened on the EVM.
//!   `ForwardThenReject` executes the request on anvil (real side effect) but
//!   loses the answer, which models "broadcast succeeded, the provider timed out".
//!
//! # Completion detection
//!
//! `PocketIc::await_call` (pocket-ic 6.0.0) ticks the server up to a fixed budget
//! (100 rounds) and, if the call is still running (for example parked on an
//! outcall nobody answers, or about to make another one), it PANICS in the client
//! with `BadIngressMessage("Failed to answer to ingress ... after 100 rounds.")`
//! rather than returning an `Err`. There is no non-blocking ingress-status API.
//! [`await_with_outcalls`] therefore only calls `await_call` after
//! [`DEFAULT_IDLE_BEFORE_AWAIT`] consecutive ticks with no pending outcall (long
//! enough to cover the inter-canister hops between two outcalls) and, as a
//! backstop, catches that specific panic and keeps pumping.

use super::anvil::Endpoint;
use pocket_ic::common::rest::{
    CanisterHttpReject, CanisterHttpReply, CanisterHttpRequest, CanisterHttpResponse,
    MockCanisterHttpResponse, RawMessageId,
};
use pocket_ic::{PocketIc, WasmResult};
use serde_json::Value;
use std::collections::HashSet;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, Ordering};

/// Fake provider base URLs the harness routes to anvil by default.
pub const PROVIDER_A: &str = "https://provider-a.test/";
pub const PROVIDER_B: &str = "https://provider-b.test/";
pub const PROVIDER_C: &str = "https://provider-c.test/";

/// Consecutive rounds with no pending outcall before [`pump_until_idle`] stops.
/// Large enough to cover the inter-canister hops between two outcalls.
const IDLE_ROUNDS: u32 = 8;

/// What to do with one provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultAction {
    /// Forward to anvil, answer 200 with anvil's body. REAL execution.
    Pass,
    /// Do not contact anvil; answer 200 with this body. MOCKED evidence.
    ReplaceBody(String),
    /// Do not contact anvil; answer with this HTTP status and a stub body. MOCKED.
    HttpStatus(u16),
    /// Do not contact anvil; reject the outcall (`CanisterHttpReject`). MOCKED.
    Reject,
    /// Do not answer this request on the round it is first seen; answer it
    /// normally (the hook is asked again, and a second `DelayOnce` counts as
    /// `Pass`) on a later round.
    DelayOnce,
    /// Forward to anvil (REAL side effect) but then reject the outcall, so the
    /// caller never learns the outcome.
    ForwardThenReject,
}

/// How a request was ultimately served (for assertions on the router log).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Served {
    Real,
    MockedBody,
    MockedStatus(u16),
    MockedReject,
    /// Forwarded to anvil, answer dropped.
    RealResponseLost,
    Delayed,
    /// URL matched no registered provider (answered 404, anvil not contacted).
    UnknownProvider,
    /// anvil itself was unreachable (answered 502).
    UpstreamError(String),
}

#[derive(Clone, Debug)]
pub struct OutcallRecord {
    pub request_id: u64,
    /// Registered provider base URL, or the raw URL for an unknown provider.
    pub provider: String,
    /// JSON-RPC method(s): one entry per call, several for a batch.
    pub rpc_methods: Vec<String>,
    pub request: Value,
    pub served: Served,
}

/// Anything that can answer a parked outcall.
pub trait OutcallRouter {
    /// `Some(response)` answers the request now; `None` leaves it parked (it is
    /// offered again on the next round).
    fn route(&mut self, req: &CanisterHttpRequest) -> Option<CanisterHttpResponse>;
}

type FaultFn = Box<dyn Fn(&str, &Value) -> FaultAction>;

/// Routes several fake provider hosts to ONE anvil endpoint, with per-provider
/// fault injection and a request log.
pub struct ProviderRouter {
    upstream: Endpoint,
    providers: Vec<String>,
    fault: Option<FaultFn>,
    delayed: HashSet<(Vec<u8>, u64)>,
    log: Vec<OutcallRecord>,
}

impl ProviderRouter {
    /// Router with the three default providers ([`PROVIDER_A`], [`PROVIDER_B`],
    /// [`PROVIDER_C`]) in front of `upstream`.
    pub fn new(upstream: Endpoint) -> ProviderRouter {
        ProviderRouter {
            upstream,
            providers: vec![
                PROVIDER_A.to_string(),
                PROVIDER_B.to_string(),
                PROVIDER_C.to_string(),
            ],
            fault: None,
            delayed: HashSet::new(),
            log: Vec::new(),
        }
    }

    /// Register one more provider base URL.
    pub fn add_provider(&mut self, base_url: &str) {
        self.providers.push(base_url.to_string());
    }

    /// Install the fault hook. It receives the registered provider base URL
    /// (for example `"https://provider-b.test/"`) and the parsed JSON-RPC request
    /// (an object, or an array for a batch).
    pub fn set_fault<F: Fn(&str, &Value) -> FaultAction + 'static>(&mut self, f: F) {
        self.fault = Some(Box::new(f));
    }

    pub fn clear_fault(&mut self) {
        self.fault = None;
    }

    pub fn log(&self) -> &[OutcallRecord] {
        &self.log
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    /// Number of logged requests to `provider` whose JSON-RPC method list
    /// contains `rpc_method` and that were actually answered (not `Delayed`).
    pub fn count(&self, provider: &str, rpc_method: &str) -> usize {
        self.log
            .iter()
            .filter(|r| r.provider == provider && r.served != Served::Delayed)
            .filter(|r| r.rpc_methods.iter().any(|m| m == rpc_method))
            .count()
    }

    fn match_provider(&self, url: &str) -> Option<String> {
        self.providers
            .iter()
            .find(|base| {
                let trimmed = base.trim_end_matches('/');
                url == trimmed
                    || url.starts_with(&format!("{trimmed}/"))
                    || url.starts_with(&format!("{trimmed}?"))
            })
            .cloned()
    }

    fn record(
        &mut self,
        req: &CanisterHttpRequest,
        provider: String,
        request: Value,
        served: Served,
    ) {
        self.log.push(OutcallRecord {
            request_id: req.request_id,
            provider,
            rpc_methods: rpc_methods(&request),
            request,
            served,
        });
    }
}

fn reply(status: u16, body: Vec<u8>) -> CanisterHttpResponse {
    CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
        status,
        headers: vec![],
        body,
    })
}

fn reject(message: &str) -> CanisterHttpResponse {
    // 2 = SYS_TRANSIENT, the code real HTTP outcall failures use.
    CanisterHttpResponse::CanisterHttpReject(CanisterHttpReject {
        reject_code: 2,
        message: message.to_string(),
    })
}

/// JSON-RPC method names in a request value (object or batch array).
pub fn rpc_methods(request: &Value) -> Vec<String> {
    match request {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v["method"].as_str().map(String::from))
            .collect(),
        v => v["method"].as_str().map(String::from).into_iter().collect(),
    }
}

impl OutcallRouter for ProviderRouter {
    fn route(&mut self, req: &CanisterHttpRequest) -> Option<CanisterHttpResponse> {
        let request: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let Some(provider) = self.match_provider(&req.url) else {
            self.record(req, req.url.clone(), request, Served::UnknownProvider);
            return Some(reply(404, b"unknown provider".to_vec()));
        };

        let key = (req.subnet_id.as_slice().to_vec(), req.request_id);
        let mut action = match &self.fault {
            Some(f) => f(&provider, &request),
            None => FaultAction::Pass,
        };
        if action == FaultAction::DelayOnce {
            if self.delayed.contains(&key) {
                // Already withheld once: answer normally now.
                action = FaultAction::Pass;
            } else {
                self.delayed.insert(key);
                self.record(req, provider, request, Served::Delayed);
                return None;
            }
        }

        let (served, response) = match action {
            FaultAction::DelayOnce => unreachable!("handled above"),
            FaultAction::Pass => match self.upstream.post(&req.body) {
                Ok((200, body)) => (Served::Real, reply(200, body)),
                Ok((status, body)) => (Served::Real, reply(status, body)),
                Err(e) => (Served::UpstreamError(e.clone()), reply(502, e.into_bytes())),
            },
            FaultAction::ReplaceBody(body) => (Served::MockedBody, reply(200, body.into_bytes())),
            FaultAction::HttpStatus(status) => (
                Served::MockedStatus(status),
                reply(status, br#"{"error":"fault injected"}"#.to_vec()),
            ),
            FaultAction::Reject => (
                Served::MockedReject,
                reject("fault injected: outcall rejected"),
            ),
            FaultAction::ForwardThenReject => {
                let _ = self.upstream.post(&req.body);
                (
                    Served::RealResponseLost,
                    reject("fault injected: response lost after forwarding"),
                )
            }
        };
        self.record(req, provider, request, served);
        Some(response)
    }
}

/// Answer every parked outcall the router is willing to answer. Returns
/// `(pending_before, answered)`.
pub fn serve_pending(pic: &PocketIc, router: &mut dyn OutcallRouter) -> (usize, usize) {
    let pending = pic.get_canister_http();
    let mut answered = 0;
    let total = pending.len();
    for req in pending {
        if let Some(response) = router.route(&req) {
            pic.mock_canister_http_response(MockCanisterHttpResponse {
                subnet_id: req.subnet_id,
                request_id: req.request_id,
                response,
                additional_responses: vec![],
            });
            answered += 1;
        }
    }
    (total, answered)
}

/// Like [`serve_pending`], but every parked request for which `hold` returns true is left
/// parked: it is offered again (and held again) on the next round, and it is never shown to
/// the router, so nothing about it is logged or faulted. Lets a test pace ONE request of a
/// running call (for example to move IC time while a canister call is mid-flight) while
/// everything else, including other requests of the same call, is answered normally. The
/// answer, once the hold is lifted, is whatever the router gives, so a held request stays
/// REAL evidence under [`FaultAction::Pass`]. Returns `(pending_total, held, answered)`.
pub fn serve_pending_except(
    pic: &PocketIc,
    router: &mut dyn OutcallRouter,
    hold: &dyn Fn(&CanisterHttpRequest) -> bool,
) -> (usize, usize, usize) {
    let pending = pic.get_canister_http();
    let total = pending.len();
    let mut held = 0;
    let mut answered = 0;
    for req in pending {
        if hold(&req) {
            held += 1;
            continue;
        }
        if let Some(response) = router.route(&req) {
            pic.mock_canister_http_response(MockCanisterHttpResponse {
                subnet_id: req.subnet_id,
                request_id: req.request_id,
                response,
                additional_responses: vec![],
            });
            answered += 1;
        }
    }
    (total, held, answered)
}

/// Submit an update call without executing it.
pub fn submit(
    pic: &PocketIc,
    canister: candid::Principal,
    sender: candid::Principal,
    method: &str,
    arg: Vec<u8>,
) -> Result<RawMessageId, String> {
    pic.submit_call(canister, sender, method, arg)
        .map_err(|e| format!("submit_call {method}: {e}"))
}

const MAX_ROUNDS: u32 = 3000;

/// Quiet ticks (no pending outcall) required before `await_call` is attempted.
pub const DEFAULT_IDLE_BEFORE_AWAIT: u32 = 6;

static STILL_RUNNING_RETRIES: AtomicU32 = AtomicU32::new(0);

/// How many times [`await_with_outcalls`] hit the `await_call` "still running"
/// panic and recovered by pumping on. Lets the smoke test prove that recovery
/// path works.
pub fn still_running_retries() -> u32 {
    STILL_RUNNING_RETRIES.load(Ordering::SeqCst)
}

enum AwaitOutcome {
    Done(Result<Vec<u8>, String>),
    StillRunning,
}

/// One `await_call` attempt, converting the "Failed to answer to ingress" panic
/// (call not finished within the server's 100-round budget) into `StillRunning`.
fn try_await(pic: &PocketIc, id: &RawMessageId) -> AwaitOutcome {
    match catch_unwind(AssertUnwindSafe(|| pic.await_call(id.clone()))) {
        Ok(Ok(WasmResult::Reply(bytes))) => AwaitOutcome::Done(Ok(bytes)),
        Ok(Ok(WasmResult::Reject(msg))) => AwaitOutcome::Done(Err(format!("rejected: {msg}"))),
        Ok(Err(e)) => AwaitOutcome::Done(Err(format!("user error: {e}"))),
        Err(payload) => {
            let text = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            if text.contains("Failed to answer to ingress") {
                AwaitOutcome::StillRunning
            } else {
                resume_unwind(payload)
            }
        }
    }
}

/// Pump outcalls until the submitted call finishes. `Ok(reply_bytes)` on a
/// reply, `Err(text)` on reject, trap or round-budget exhaustion.
pub fn await_with_outcalls(
    pic: &PocketIc,
    id: RawMessageId,
    router: &mut dyn OutcallRouter,
) -> Result<Vec<u8>, String> {
    await_with_outcalls_opts(pic, id, router, DEFAULT_IDLE_BEFORE_AWAIT)
}

/// [`await_with_outcalls`] with an explicit quiet-tick threshold (0 attempts
/// `await_call` on the first quiet tick; used by the smoke test to prove the
/// recovery path).
pub fn await_with_outcalls_opts(
    pic: &PocketIc,
    id: RawMessageId,
    router: &mut dyn OutcallRouter,
    idle_before_await: u32,
) -> Result<Vec<u8>, String> {
    let mut idle = 0u32;
    for _round in 0..MAX_ROUNDS {
        pic.tick();
        let (pending, _answered) = serve_pending(pic, router);
        if pending > 0 {
            idle = 0;
            continue;
        }
        idle += 1;
        if idle < idle_before_await {
            continue;
        }
        match try_await(pic, &id) {
            AwaitOutcome::Done(result) => return result,
            AwaitOutcome::StillRunning => {
                STILL_RUNNING_RETRIES.fetch_add(1, Ordering::SeqCst);
                idle = 0;
            }
        }
    }
    Err(format!("call did not finish within {MAX_ROUNDS} rounds"))
}

/// `submit_call` + [`await_with_outcalls`]. The arg is already Candid-encoded
/// (`candid::encode_args(..)`), and so is the returned reply.
pub fn call_with_outcalls(
    pic: &PocketIc,
    canister: candid::Principal,
    sender: candid::Principal,
    method: &str,
    arg_bytes: Vec<u8>,
    router: &mut dyn OutcallRouter,
) -> Result<Vec<u8>, String> {
    let id = submit(pic, canister, sender, method, arg_bytes)?;
    await_with_outcalls(pic, id, router)
}

/// Drive timer-driven work: tick and answer outcalls until `IDLE_ROUNDS`
/// consecutive rounds show no pending outcall (or `max_rounds` is reached).
/// Advance PocketIC time first (`pic.advance_time(..)`) so the timers are due.
/// Returns the number of rounds executed.
pub fn pump_until_idle(pic: &PocketIc, router: &mut dyn OutcallRouter, max_rounds: u32) -> u32 {
    let mut idle = 0;
    for round in 1..=max_rounds {
        pic.tick();
        let (pending, _) = serve_pending(pic, router);
        if pending == 0 {
            idle += 1;
            if idle >= IDLE_ROUNDS {
                return round;
            }
        } else {
            idle = 0;
        }
    }
    max_rounds
}
