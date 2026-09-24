use super::*;

/// The loop's provider seam (spec §6): a turn is a request in, a result out,
/// with every stream delta forwarded to the sink on the way — the live path
/// and the canned test path share this contract (ticket #19).
pub type ProviderTurn<'a> =
    Pin<Box<dyn Future<Output = Result<TurnResult, ProviderError>> + Send + 'a>>;

pub trait TurnProvider: Send + Sync {
    fn call<'a>(&self, request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a>;
}

pub type TurnProviderRef = Arc<dyn TurnProvider>;

/// The production seam: the live responses endpoint (spec §6).
pub fn production(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
) -> TurnProviderRef {
    Arc::new(ProductionProvider {
        client: client.clone(),
        provider: provider.clone(),
        requests: requests.clone(),
    })
}

struct ProductionProvider {
    client: reqwest::Client,
    provider: Provider,
    requests: Requests,
}

impl TurnProvider for ProductionProvider {
    fn call<'a>(&self, request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let client = self.client.clone();
        let provider = self.provider.clone();
        let requests = self.requests.clone();
        let request = request.clone();
        Box::pin(async move { stream_turn(&client, &provider, &requests, &request, sink).await })
    }
}

/// A canned provider for tests (ticket #19's SSE-fixture seam): replays the
/// events decoded from a canned SSE body through the same sink contract as
/// the live path, so lane semantics run against the real decode pipeline.
struct CannedProvider {
    events: Vec<TurnEvent>,
    /// Calls indexed by the event position they completed at.
    calls: Vec<(usize, FunctionCall)>,
    /// Cut the stream after this many events (the force-kill shape);
    /// `usize::MAX` = the full stream.
    cut_after: usize,
}

impl TurnProvider for CannedProvider {
    fn call<'a>(&self, _request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let events = self.events.clone();
        let calls = self.calls.clone();
        let cut_after = self.cut_after;
        Box::pin(async move {
            let mut result = TurnResult::default();
            let mut accepted = 0usize;
            for event in &events {
                if accepted >= cut_after {
                    break;
                }
                if !sink.event(event.clone()) {
                    break;
                }
                fold_event(event, &mut result);
                accepted += 1;
            }
            if accepted == events.len() {
                result.completed = events.iter().any(|e| matches!(e, TurnEvent::Completed(_)));
            }
            result.calls = calls
                .iter()
                .filter(|(i, _)| *i <= accepted)
                .map(|(_, c)| c.clone())
                .collect();
            Ok(result)
        })
    }
}

/// A canned provider replaying the full decoded stream.
pub fn canned(body: &str) -> TurnProviderRef {
    let (events, calls) = decode_stream(body).expect("canned SSE body must decode");
    Arc::new(CannedProvider {
        events,
        calls,
        cut_after: usize::MAX,
    })
}

/// A canned provider whose stream is cut after `n` events — the shape a
/// force-kill produces (partial, `completed: false`).
pub fn canned_cut(body: &str, n: usize) -> TurnProviderRef {
    let (events, calls) = decode_stream(body).expect("canned SSE body must decode");
    Arc::new(CannedProvider {
        events,
        calls,
        cut_after: n,
    })
}

/// A canned provider that sleeps between events — the in-flight seam for
/// force-kill tests: the loop's kill flag, not a pre-cut, terminates it.
struct SlowCannedProvider {
    events: Vec<TurnEvent>,
    delay_ms: u64,
}

impl TurnProvider for SlowCannedProvider {
    fn call<'a>(&self, _request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let events = self.events.clone();
        let delay_ms = self.delay_ms;
        Box::pin(async move {
            let mut result = TurnResult::default();
            let mut accepted = 0usize;
            for event in &events {
                if !sink.event(event.clone()) {
                    break;
                }
                fold_event(event, &mut result);
                accepted += 1;
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            if accepted == events.len() {
                result.completed = events.iter().any(|e| matches!(e, TurnEvent::Completed(_)));
            }
            Ok(result)
        })
    }
}

/// A canned provider replaying its stream with a delay between events: a
/// force sent mid-stream cuts it (the loop's kill flag is the terminator).
pub fn canned_slow(body: &str, delay_ms: u64) -> TurnProviderRef {
    let (events, _calls) = decode_stream(body).expect("canned SSE body must decode");
    Arc::new(SlowCannedProvider { events, delay_ms })
}
/// A provider entry with this base URL is a scripted stream, not an HTTP
/// endpoint: the `canned()` test seam promoted to a provider entry so the
/// GUI's real-app E2E can drive deterministic turns through the real core.
/// A documented test hook, not a product surface — debug builds only
/// (roadmap G, the 25 ms coalesced-stream acceptance bar).
#[cfg(debug_assertions)]
pub const CANNED_SCHEME: &str = "canned://";

#[cfg(debug_assertions)]
fn script_text() -> String {
    let mut body = String::new();
    for i in 0..40 {
        body.push_str(&format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"word {i} \"}}\n\n"
        ));
    }
    body.push_str(
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":40,\"total_tokens\":140}}}\n\ndata: [DONE]\n\n",
    );
    body
}
#[cfg(debug_assertions)]
fn script_reasoning() -> String {
    let mut body = String::new();
    for i in 0..20 {
        body.push_str(&format!(
            "data: {{\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"thought {i} \"}}\n\n"
        ));
    }
    for i in 0..20 {
        body.push_str(&format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"word {i} \"}}\n\n"
        ));
    }
    body.push_str(
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":40,\"total_tokens\":140}}}\n\ndata: [DONE]\n\n",
    );
    body
}

/// Resolve a `canned://` provider entry to its scripted replay at the 25 ms
/// cadence. Debug builds only: release returns `None` and the caller must
/// refuse the entry (a `canned://` URL is not a reachable endpoint).
pub fn canned_dev(provider: &Provider) -> Option<TurnProviderRef> {
    #[cfg(debug_assertions)]
    {
        let script = provider.base_url.strip_prefix(CANNED_SCHEME)?;
        let body = match script {
            "text" => script_text(),
            "reasoning" => script_reasoning(),
            _ => return None,
        };
        Some(canned_slow(&body, 25))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = provider;
        None
    }
}
