use super::{
    CompletedResponse, Duration, Frame, FunctionCall, OutputItem, Provider, ProviderError,
    Requests, ResponseRequest, SseParser, StreamOutcome, TurnEvent, TurnResult, TurnSink, Usage,
    endpoint_url, fold_event, resolve_key,
};

fn emit(sink: &mut dyn TurnSink, result: &mut TurnResult, event: &TurnEvent) -> bool {
    if !sink.event(event.clone()) {
        return false;
    }
    fold_event(event, result);
    true
}

/// How a response ended, as told by the frames and by the body. The first
/// terminal frame is the provider's own account and wins; the body's own
/// ending — `[DONE]`, a stop, an idle deadline — holds only where no frame
/// named one; an error frame that neither ended the stream nor was followed by
/// a terminal frame names a mid-stream error; and a body that simply stopped
/// names nothing, which is `EofWithoutDone` (#307).
#[derive(Default)]
pub(super) struct Ending {
    terminal: Option<StreamOutcome>,
    body: Option<StreamOutcome>,
    errored: bool,
}

impl Ending {
    fn terminal(&mut self, ending: StreamOutcome) {
        self.terminal.get_or_insert(ending);
    }
    fn body(&mut self, ending: StreamOutcome) {
        if self.body.is_none() {
            self.body = Some(ending);
        }
    }
    fn error(&mut self) {
        self.errored = true;
    }

    fn resolve(self) -> StreamOutcome {
        if let Some(ending) = self.terminal {
            return ending;
        }
        if let Some(ending) = self.body {
            return ending;
        }
        if self.errored {
            StreamOutcome::MidStreamError
        } else {
            StreamOutcome::EofWithoutDone
        }
    }
}

/// Consume one `data:` payload: accumulate into the result and forward the
/// stream-delta events to the sink (false = kill, spec §7).
/// `created_at` is the running `response.created` timestamp (#224), threaded
/// by the caller: a per-payload function holds no cross-frame state, so the
/// decode-window duration is computed here, at the `response.completed`
/// frame, from the two server timestamps. `ending` records how the response
/// ended, in the frames' own words where they have any.
pub(super) fn apply_frame(
    sink: &mut dyn TurnSink,
    result: &mut TurnResult,
    created_at: &mut Option<i64>,
    ending: &mut Ending,
    payload: &str,
) -> bool {
    // Undecodable frames are skipped rather than failing the turn: servers
    // pad the stream with non-JSON frames (research #4, quirk 1).
    let frame: Frame = match serde_json::from_str(payload) {
        Ok(frame) => frame,
        Err(_) => return true,
    };
    // The JSONL response line's `blocks` (the #98 debug view): the frame's
    // content blocks in arrival order, so the log shows where each tool call
    // fell in the model's reasoning. `debug_assertions` is true in dev and
    // test builds, false in release — the per-frame clones never compile in.
    #[cfg(debug_assertions)]
    {
        result.blocks.extend(frame.wire_blocks());
    }
    match frame {
        Frame::Created { created_at: ts } => {
            if let Some(ts) = ts {
                *created_at = Some(ts);
            }
            true
        }
        Frame::Completed { response } => {
            ending.terminal(StreamOutcome::Completed);
            settle_turn(sink, result, created_at, response)
        }
        Frame::OutputTextDelta {
            delta,
            reasoning_text,
            reasoning_details,
        } => {
            if let Some(text) = delta
                && !emit(sink, result, &TurnEvent::Text(text))
            {
                return false;
            }
            if let Some(text) = reasoning_text
                && !emit(sink, result, &TurnEvent::Reasoning(text))
            {
                return false;
            }
            for block in reasoning_details.into_iter().flatten() {
                if let Some(text) = block.text
                    && !emit(sink, result, &TurnEvent::Reasoning(text))
                {
                    return false;
                }
            }
            true
        }
        Frame::ReasoningDelta { delta } | Frame::ReasoningTextDelta { delta } => {
            if let Some(text) = delta {
                emit(sink, result, &TurnEvent::Reasoning(text))
            } else {
                true
            }
        }
        Frame::OutputItemDone { item } => {
            if let OutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } = item
            {
                result.calls.push(FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                });
            }
            true
        }
        Frame::Error { error } => {
            // A frame naming an error is the provider saying the stream went
            // wrong. It does not end the stream — a later terminal frame still
            // outranks it (#307).
            ending.error();
            result.mid_stream_errors.push(
                error
                    .message
                    .clone()
                    .unwrap_or_else(|| "mid-stream provider error".into()),
            );
            true
        }
        // Either terminal frame settles the call and carries its usage; they
        // differ only in how the response ended.
        Frame::Incomplete { response } => {
            ending.terminal(if output_cap(response.as_ref()) {
                StreamOutcome::Length
            } else {
                StreamOutcome::EofWithoutDone
            });
            settle_turn(sink, result, created_at, response)
        }
        Frame::InProgress | Frame::Other => true,
    }
}

/// Settle a terminal frame: the decode-window duration, then the usage the
/// frame carries (#224). The duration is the server's own measurement —
/// completion minus creation — and is `None` when either timestamp is absent
/// (the GUI then falls back to its wall-clock estimate).
fn settle_turn(
    sink: &mut dyn TurnSink,
    result: &mut TurnResult,
    created_at: &mut Option<i64>,
    response: Option<CompletedResponse>,
) -> bool {
    let duration = response
        .as_ref()
        .and_then(|r| r.created_at)
        .zip(*created_at)
        .and_then(|(end, start)| {
            let secs = u64::try_from(end - start).ok()?;
            // A zero-length window means the server stamped created
            // and completed with the same second (vLLM does —
            // research/tps-server-timestamps.md); treat it as absent
            // so the GUI falls back to its wall-clock estimate.
            (secs > 0).then(|| Duration::from_secs(secs))
        });
    match response.and_then(|r| r.usage) {
        Some(wire) => emit(
            sink,
            result,
            &TurnEvent::Completed(wire.into_usage(), duration),
        ),
        None => true,
    }
}

/// Whether an incomplete response stopped at the output cap: the one stop
/// reason the outcome vocabulary names.
fn output_cap(response: Option<&CompletedResponse>) -> bool {
    response
        .and_then(|r| r.incomplete_details.as_ref())
        .and_then(|d| d.reason.as_deref())
        == Some("max_output_tokens")
}

/// Decode a canned SSE body into the event/call sequence the live path would
/// produce (the canned provider's source; also the offline-decode utility).
/// A canned stream: the events in order, plus each call with the event
/// position it completed at, and how the body ended (the ending a live
/// provider would have reported for the same bytes).
pub type CannedStream = (Vec<TurnEvent>, Vec<(usize, FunctionCall)>, StreamOutcome);

pub fn decode_stream(body: &str) -> Result<CannedStream, ProviderError> {
    struct RecordSink(Vec<TurnEvent>);
    impl TurnSink for RecordSink {
        fn event(&mut self, event: TurnEvent) -> bool {
            self.0.push(event);
            true
        }
    }
    let mut parser = SseParser::new();
    let payloads = parser.feed(body.as_bytes())?;
    let mut sink = RecordSink(Vec::new());
    let mut result = TurnResult::default();
    let mut created_at: Option<i64> = None;
    let mut ending = Ending::default();
    let mut calls = Vec::new();
    let mut seen_calls = 0usize;
    for payload in payloads {
        if payload == "[DONE]" {
            break;
        }
        apply_frame(
            &mut sink,
            &mut result,
            &mut created_at,
            &mut ending,
            &payload,
        );
        while result.calls.len() > seen_calls {
            let call = result
                .calls
                .last()
                .cloned()
                .expect("calls appended in order");
            calls.push((sink.0.len(), call));
            seen_calls += 1;
        }
    }
    // A body that ended with [DONE] is a clean end: it stands as complete even
    // when the server omitted the response.completed frame (some do, on
    // tool-call responses). Synthesize the completion so the canned replay
    // mirrors the live path's clean_end; a [DONE]-less cut body reaches no
    // completion, as before.
    if parser.terminated {
        ending.body(StreamOutcome::Completed);
    }
    result.outcome = ending.resolve();
    if result.outcome == StreamOutcome::Completed
        && !sink
            .0
            .iter()
            .any(|e| matches!(e, TurnEvent::Completed(_, _)))
    {
        sink.0.push(TurnEvent::Completed(Usage::default(), None));
    }
    Ok((sink.0, calls, result.outcome))
}

/// One streamed `POST {base}/responses` turn (spec §6): typed SSE frames,
/// reasoning-dialect normalization, usage from `response.completed`, and the
/// delta events on the sink. Retries connect/timeout/5xx up to
/// `requests.retries` with exponential backoff; 4xx and mid-stream errors
/// fail or annotate the turn without retrying.
pub async fn stream_turn(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
    request: &ResponseRequest,
    sink: &mut dyn TurnSink,
) -> Result<TurnResult, ProviderError> {
    let url = endpoint_url(&provider.base_url, "responses");
    let key = resolve_key(provider);
    let mut attempt = 0u32;
    loop {
        match attempt_one_turn(client, &url, key.as_deref(), requests, request, sink).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let retryable = match &e {
                    ProviderError::Request(reqwest_err) => {
                        reqwest_err.is_connect() || reqwest_err.is_timeout()
                    }
                    // A dead stream before its body is a transport failure:
                    // retry it, like a connect timeout.
                    ProviderError::IdleTimeout => true,
                    ProviderError::Status { status, .. } => (500..599).contains(status),
                    ProviderError::MalformedStream(_) => false,
                };
                if !retryable || attempt >= requests.retries {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(6))).await;
                attempt += 1;
            }
        }
    }
}

/// One streamed turn; the events are discarded (the original #17 surface).
pub async fn stream_response(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
    request: &ResponseRequest,
) -> Result<TurnResult, ProviderError> {
    struct Keep;
    impl TurnSink for Keep {
        fn event(&mut self, _: TurnEvent) -> bool {
            true
        }
    }
    stream_turn(client, provider, requests, request, &mut Keep).await
}

/// The headers phase, apart from the body loop: the request answered, cut by
/// a stop before any answer, or failed. A phase silent past the deadline is a
/// dead endpoint: fail (and retry), like a connect timeout.
enum Head {
    Answered(reqwest::Response),
    Cut,
    Failed(ProviderError),
}

async fn open_head(
    builder: reqwest::RequestBuilder,
    sink: &mut dyn TurnSink,
    head: Duration,
) -> Head {
    let mut sending = builder.send();
    match tokio::select! {
        sent = tokio::time::timeout_at(tokio::time::Instant::now() + head, &mut sending) => sent,
        () = sink.stop_signal() => return Head::Cut,
    } {
        Ok(Ok(response)) => {
            let status = response.status().as_u16();
            if response.status().is_success() {
                Head::Answered(response)
            } else {
                let body = response.text().await.unwrap_or_default();
                Head::Failed(ProviderError::Status { status, body })
            }
        }
        Ok(Err(e)) => Head::Failed(ProviderError::Request(e)),
        Err(_) => Head::Failed(ProviderError::IdleTimeout),
    }
}

async fn attempt_one_turn(
    client: &reqwest::Client,
    url: &str,
    key: Option<&str>,
    requests: &Requests,
    request: &ResponseRequest,
    sink: &mut dyn TurnSink,
) -> Result<TurnResult, ProviderError> {
    // The JSONL request/response pairing (the #98 debug view) is `debug!`
    // level: `level_enabled!` folds to a compile-time const under a
    // `max_level_*` feature, so a release build (the app sets
    // `release_max_level_error`) drops the id mint and the timestamp — only
    // the trace-level verbatim dump (self-guarded by `trace!`) survives.
    let wire_id: Option<String> = if tracing::level_enabled!(tracing::Level::DEBUG) {
        Some(super::wire::new_id())
    } else {
        None
    };
    let wire_start: Option<tokio::time::Instant> =
        wire_id.as_ref().map(|_| tokio::time::Instant::now());
    super::wire::request(request);
    if let Some(id) = &wire_id {
        request.wire_request_jsonl(id);
    }
    // `.timeout` is a *total* deadline, which kills a healthy long stream
    // mid-body: the connect/headers phase is bounded by
    // `requests.timeout_secs`, the stream by the per-chunk idle deadline
    // (reset on every chunk). A silent stream is dead; a slow one is not.
    let head = Duration::from_secs(requests.timeout_secs);
    let idle = Duration::from_secs(requests.idle_timeout_secs);
    let mut builder = client.post(url).json(request);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    let mut stream = match open_head(builder, sink, head).await {
        Head::Answered(response) => response,
        // A stop before the headers tears the request down before it is
        // answered: nothing received, the empty partial stands (spec §7).
        Head::Cut => {
            return Ok(TurnResult {
                outcome: StreamOutcome::Killed,
                ..Default::default()
            });
        }
        Head::Failed(e) => return Err(e),
    };
    let mut parser = SseParser::new();
    let mut result = TurnResult::default();
    let mut created_at: Option<i64> = None;
    let mut idle_deadline = tokio::time::Instant::now() + idle;
    // How the body ended, below whatever frames arrived. [DONE] means the
    // server finished the response, and that stands as complete even when it
    // omitted the response.completed frame (some do, on tool-call responses) —
    // otherwise every normal agentic segment would read as cut. A frame that
    // names an ending outranks this (#307).
    let mut ending = Ending::default();
    let mut raw_response = String::new();
    'outer: while !parser.terminated {
        let chunk = match tokio::select! {
            got = tokio::time::timeout_at(
                // `Box::pin` makes the future `Unpin`: reqwest's async-fn
                // future is not, and `timeout_at` demands it.
                idle_deadline,
                Box::pin(stream.chunk()),
            ) => got,
            // A stop during prefill (before the first token) aborts the
            // request: the partial — possibly empty — stands, like a kill.
            () = sink.stop_signal() => {
                ending.body(StreamOutcome::Killed);
                break 'outer;
            },
        } {
            Ok(Ok(Some(chunk))) => {
                // A received chunk restarts the idle deadline.
                idle_deadline = tokio::time::Instant::now() + idle;
                // The trace-level wire log (wire::response) carries the raw
                // SSE bytes: the buffer is live at `trace` level only.
                if tracing::level_enabled!(tracing::Level::TRACE) {
                    raw_response.push_str(&String::from_utf8_lossy(&chunk));
                }
                chunk
            }
            // A bare EOF (no [DONE]) is byte-identical to a mid-body drop
            // with a graceful FIN, so it stays a cut (incomplete) — the
            // canned path's [DONE]-only semantics. A compliant Responses-API
            // provider always ends with [DONE] (handled below), which names
            // the body's ending; a provider omitting both [DONE] and
            // response.completed is a contract violation and the safer
            // failure mode is an incomplete turn, not a fake-complete one.
            Ok(Ok(None)) => break,
            // A mid-body drop keeps the partial as an incomplete turn
            // instead of an error (ticket #19, #17 handoff gap c): the
            // partial cannot be re-derived by a retry; a pre-body
            // failure (nothing received) still fails and retries.
            Ok(Err(e)) => {
                if result.text.is_empty() && result.reasoning.is_empty() && result.calls.is_empty()
                {
                    return Err(ProviderError::Request(e));
                }
                break;
            }
            // The stream went silent past the idle deadline: a dead
            // stream. Nothing received fails (and retries); a received
            // partial stands as an incomplete turn, like a mid-body drop.
            Err(_) => {
                if result.text.is_empty() && result.reasoning.is_empty() && result.calls.is_empty()
                {
                    return Err(ProviderError::IdleTimeout);
                }
                ending.body(StreamOutcome::IdleTimeout);
                break;
            }
        };
        for payload in parser.feed(&chunk)? {
            if payload == "[DONE]" {
                ending.body(StreamOutcome::Completed);
                break 'outer;
            }
            if !apply_frame(sink, &mut result, &mut created_at, &mut ending, &payload) {
                // Stopped mid-body: the partial stands (spec §7 force) unless
                // the provider named the ending first (#307).
                ending.body(StreamOutcome::Killed);
                result.outcome = ending.resolve();
                return Ok(result);
            }
        }
    }
    result.outcome = ending.resolve();
    super::wire::response(&raw_response);
    // `unwrap_or(MAX)`: a 584-year stream is a diagnostic-log sentinel, not a
    // failure — `as u64` is a silent conversion, which clippy denies.
    if let (Some(id), Some(start)) = (&wire_id, &wire_start) {
        result.wire_response_jsonl(
            id,
            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
    }
    Ok(result)
}
