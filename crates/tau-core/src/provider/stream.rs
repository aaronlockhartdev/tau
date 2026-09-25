use super::*;

fn emit(sink: &mut dyn TurnSink, result: &mut TurnResult, event: TurnEvent) -> bool {
    if !sink.event(event.clone()) {
        return false;
    }
    fold_event(&event, result);
    true
}

/// Consume one `data:` payload: accumulate into the result and forward the
/// stream-delta events to the sink (false = kill, spec §7).
pub(super) fn apply_frame(sink: &mut dyn TurnSink, result: &mut TurnResult, payload: &str) -> bool {
    // Undecodable frames are skipped rather than failing the turn: servers
    // pad the stream with non-JSON frames (research #4, quirk 1).
    let frame: Frame = match serde_json::from_str(payload) {
        Ok(frame) => frame,
        Err(_) => return true,
    };
    match frame {
        Frame::Completed { response } => {
            result.completed = true;
            match response.and_then(|r| r.usage) {
                Some(usage) => emit(sink, result, TurnEvent::Completed(usage)),
                None => true,
            }
        }
        Frame::OutputTextDelta {
            delta,
            reasoning_text,
            reasoning_details,
        } => {
            if let Some(text) = delta
                && !emit(sink, result, TurnEvent::Text(text))
            {
                return false;
            }
            if let Some(text) = reasoning_text
                && !emit(sink, result, TurnEvent::Reasoning(text))
            {
                return false;
            }
            for block in reasoning_details.into_iter().flatten() {
                if let Some(text) = block.text
                    && !emit(sink, result, TurnEvent::Reasoning(text))
                {
                    return false;
                }
            }
            true
        }
        Frame::ReasoningDelta { delta } | Frame::ReasoningTextDelta { delta } => {
            if let Some(text) = delta {
                emit(sink, result, TurnEvent::Reasoning(text))
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
            result.mid_stream_errors.push(
                error
                    .message
                    .clone()
                    .unwrap_or_else(|| "mid-stream provider error".into()),
            );
            true
        }
        Frame::Created | Frame::InProgress | Frame::Other => true,
    }
}

/// Decode a canned SSE body into the event/call sequence the live path would
/// produce (the canned provider's source; also the offline-decode utility).
/// A canned stream: the events in order, plus each call with the event
/// position it completed at.
pub type CannedStream = (Vec<TurnEvent>, Vec<(usize, FunctionCall)>);

pub fn decode_stream(body: &str) -> Result<CannedStream, ProviderError> {
    let mut parser = SseParser::new();
    let payloads = parser.feed(body.as_bytes())?;
    struct RecordSink(Vec<TurnEvent>);
    impl TurnSink for RecordSink {
        fn event(&mut self, event: TurnEvent) -> bool {
            self.0.push(event);
            true
        }
    }
    let mut sink = RecordSink(Vec::new());
    let mut result = TurnResult::default();
    let mut calls = Vec::new();
    let mut seen_calls = 0usize;
    for payload in payloads {
        if payload == "[DONE]" {
            break;
        }
        apply_frame(&mut sink, &mut result, &payload);
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
    Ok((sink.0, calls))
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
                    _ => false,
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

async fn attempt_one_turn(
    client: &reqwest::Client,
    url: &str,
    key: Option<&str>,
    requests: &Requests,
    request: &ResponseRequest,
    sink: &mut dyn TurnSink,
) -> Result<TurnResult, ProviderError> {
    // `.timeout` is a *total* deadline, which kills a healthy long stream
    // mid-body: the connect is bounded by the client's connect timeout,
    // and the stream by this per-chunk idle deadline (reset on every
    // chunk). A silent stream is dead; a slow one is not.
    let idle = Duration::from_secs(requests.timeout_secs);
    let mut builder = client.post(url).json(request);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    // A connect/headers phase silent past the idle deadline is a dead
    // endpoint: fail (and retry), like a connect timeout.
    let mut sending = builder.send();
    let response = tokio::time::timeout_at(tokio::time::Instant::now() + idle, &mut sending)
        .await
        .map_err(|_| ProviderError::IdleTimeout)?
        .map_err(ProviderError::Request)?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status, body });
    }
    let mut parser = SseParser::new();
    let mut result = TurnResult::default();
    let mut stream = response;
    let mut idle_deadline = tokio::time::Instant::now() + idle;
    'outer: while !parser.terminated {
        let chunk = match tokio::time::timeout_at(
            // `Box::pin` makes the future `Unpin`: reqwest's async-fn
            // future is not, and `timeout_at` demands it.
            idle_deadline,
            Box::pin(stream.chunk()),
        )
        .await
        {
            Ok(Ok(Some(chunk))) => {
                // A received chunk restarts the idle deadline.
                idle_deadline = tokio::time::Instant::now() + idle;
                chunk
            }
            // EOF: the result stands as accumulated — `completed` only if
            // `response.completed` arrived (spec §6, #17 handoff gap b).
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
                break;
            }
        };
        for payload in parser.feed(&chunk)? {
            if payload == "[DONE]" {
                break 'outer;
            }
            if !apply_frame(sink, &mut result, &payload) {
                // Killed: the partial stands (spec §7 force).
                return Ok(result);
            }
        }
    }
    Ok(result)
}
