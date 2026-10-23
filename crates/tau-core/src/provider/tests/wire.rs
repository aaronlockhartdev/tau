use super::*;

#[test]
fn request_wire_shape_carries_tools_and_limits() {
    let request = ResponseRequest::new(
        "m",
        Some("sys"),
        vec![InputMessage {
            role: "user".into(),
            content: "hi".into(),
        }]
        .into_iter()
        .map(InputEntry::Message)
        .collect(),
    )
    .with_tools(vec![ToolSpec {
        kind: "function".into(),
        name: "bash".into(),
        description: "run".into(),
        parameters: serde_json::json!({"type": "object"}),
    }])
    .with_max_output_tokens(128)
    .with_reasoning(ReasoningEffort::Low);
    let wire: serde_json::Value = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["stream"], true);
    assert_eq!(wire["tools"][0]["type"], "function");
    assert_eq!(wire["tools"][0]["name"], "bash");
    assert_eq!(wire["max_output_tokens"], 128);
    assert_eq!(wire["reasoning"]["effort"], "low");
    assert_eq!(wire["input"][0]["role"], "user");
}

/// #34: a text tool result stays a bare string on the wire; an image block
/// rides as a content-parts array with the base64 payload in a data URL.
#[test]
fn function_call_output_carries_text_and_image_shapes() {
    let text: serde_json::Value = serde_json::to_value(&FunctionCallOutputInput {
        kind: "function_call_output",
        call_id: "c1".into(),
        output: CallOutput::Text("ok".into()),
    })
    .unwrap();
    assert_eq!(text["output"], "ok");

    let image: serde_json::Value = serde_json::to_value(&FunctionCallOutputInput {
        kind: "function_call_output",
        call_id: "c2".into(),
        output: CallOutput::Image(vec![InputCallPart::Image(InputImagePart {
            kind: "input_image",
            image_url: "data:image/png;base64,AAAA".into(),
            detail: "auto",
        })]),
    })
    .unwrap();
    assert_eq!(image["output"][0]["type"], "input_image");
    assert_eq!(
        image["output"][0]["image_url"],
        "data:image/png;base64,AAAA"
    );
    // #79: the vLLM build rejects parts without `detail`.
    assert_eq!(image["output"][0]["detail"], "auto");
}

/// #34: the payload's image block becomes the data-URL part; a string passes
/// through; no other shape is produced by the tools.
#[test]
fn call_output_from_payload_maps_the_stored_shapes() {
    let block = serde_json::json!({
        "type": "image",
        "media_type": "image/png",
        "data_base64": "AAAA"
    });
    assert_eq!(
        CallOutput::from_payload(&block),
        Some(CallOutput::Image(vec![InputCallPart::Image(
            InputImagePart {
                kind: "input_image",
                image_url: "data:image/png;base64,AAAA".into(),
                detail: "auto",
            }
        )]))
    );
    assert_eq!(
        CallOutput::from_payload(&serde_json::json!("plain")),
        Some(CallOutput::Text("plain".into()))
    );
    assert_eq!(
        CallOutput::from_payload(&serde_json::json!({"other": 1})),
        None
    );
    // #257: a noted image block rides as a parts array — the note first as an
    // `input_text` part, then the image.
    let noted = serde_json::json!({
        "type": "image",
        "media_type": "image/jpeg",
        "data_base64": "BBBB",
        "note": "[Image: original 2048x2048, displayed at 1448x1448.]"
    });
    assert_eq!(
        CallOutput::from_payload(&noted),
        Some(CallOutput::Image(vec![
            InputCallPart::Text("[Image: original 2048x2048, displayed at 1448x1448.]".into()),
            InputCallPart::Image(InputImagePart {
                kind: "input_image",
                image_url: "data:image/jpeg;base64,BBBB".into(),
                detail: "auto",
            }),
        ]))
    );
}

#[test]
fn request_wire_shape_carries_generation_thinking_and_cache() {
    let request = ResponseRequest::new("m", Some("sys"), vec![])
        .with_max_output_tokens(8192)
        .with_temperature(0.5)
        .with_top_p(0.9375)
        .with_frequency_penalty(0.25)
        .with_presence_penalty(0.0)
        .with_reasoning(ReasoningEffort::High)
        .with_reasoning_summary()
        .with_prompt_cache("sess-1".into(), crate::config::CacheRetention::Long);
    let wire: serde_json::Value = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["max_output_tokens"], 8192);
    assert_eq!(wire["temperature"], 0.5);
    assert_eq!(wire["top_p"], 0.9375);
    assert_eq!(wire["frequency_penalty"], 0.25);
    assert_eq!(wire["presence_penalty"], 0.0);
    assert_eq!(wire["reasoning"]["effort"], "high");
    assert_eq!(wire["reasoning"]["summary"], "auto");
    assert_eq!(wire["prompt_cache_key"], "sess-1");
    assert_eq!(wire["prompt_cache_options"]["type"], "default");
    assert_eq!(wire["prompt_cache_options"]["retention"], "7d");
}

#[test]
fn request_wire_shape_omits_unset_options() {
    let wire: serde_json::Value =
        serde_json::to_value(ResponseRequest::new("m", None, vec![])).unwrap();
    assert!(wire.get("temperature").is_none());
    assert!(wire.get("top_p").is_none());
    assert!(wire.get("frequency_penalty").is_none());
    assert!(wire.get("presence_penalty").is_none());
    assert!(wire.get("reasoning").is_none());
    assert!(wire.get("prompt_cache_key").is_none());
    assert!(wire.get("prompt_cache_options").is_none());
}

#[test]
fn clamp_respects_the_declared_window_and_prompt() {
    // Cap below the window's remainder: untouched.
    assert_eq!(clamp_max_output(8192, Some(32768), 1000), 8192);
    // Cap above the remainder: clamped to it.
    assert_eq!(clamp_max_output(8192, Some(9000), 1000), 8000);
    // No declared window: untouched.
    assert_eq!(clamp_max_output(8192, None, 1000), 8192);
    // Prompt fills the window: untouched (the server rejects).
    assert_eq!(clamp_max_output(8192, Some(500), 1000), 8192);
}

#[test]
fn prompt_estimate_counts_image_data_urls_and_text() {
    // #255: an image call output must count toward the gate's budget —
    // the data URL and any text parts, not zero.
    let url = "data:image/png;base64,".to_owned() + &"A".repeat(4000);
    let input = vec![InputEntry::CallOutput(FunctionCallOutputInput {
        kind: "function_call_output",
        call_id: "c1".into(),
        output: CallOutput::Image(vec![
            InputCallPart::Text("note".to_owned()),
            InputCallPart::Image(InputImagePart {
                kind: "input_image",
                image_url: url.clone(),
                detail: "auto",
            }),
        ]),
    })];
    let est = prompt_token_estimate("sys", &input, &[]);
    // ~4 chars/token (the shared soft estimator, spec §4): 4022 chars / 4
    // for the URL, 1 for the note ("sys" is under a token).
    assert_eq!(est, 4022 / 4 + 1, "image and note both count");
}

#[test]
fn prompt_estimate_counts_text_call_outputs() {
    // The pre-#255 behavior for text: unchanged by the image fix.
    let input = vec![InputEntry::CallOutput(FunctionCallOutputInput {
        kind: "function_call_output",
        call_id: "c1".into(),
        output: CallOutput::Text("b".repeat(1200)),
    })];
    let est = prompt_token_estimate("sys", &input, &[]);
    // The "sys" prompt is under a token: only the result counts.
    assert_eq!(est, 1200 / 4);
}

// --- JSONL wire view (the #98 wire log's parseable companion) ---

/// One static buffer per test binary: `set_global_default` installs once,
/// so every `llm wire` event lands here; each scenario drains it first.
static CAPTURE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// A `MakeWriter` over the static buffer: the subscriber holds this unit
/// struct, and each event locks the shared buffer.
struct CapSource;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapSource {
    type Writer = CapWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        CapWriter(CAPTURE.lock().expect("capture poisoned"))
    }
}

/// A `Write` view over one locked buffer: `MutexGuard<String>` is not
/// itself a writer.
struct CapWriter<'a>(std::sync::MutexGuard<'a, String>);

impl std::io::Write for CapWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Install the capture once per binary, run `f`, and return what it emitted.
/// Each call drains the buffer first, so a scenario only sees its own event.
/// `try_init` succeeds only on the first call; the later no-op calls still
/// write into the same static buffer, so the read is unconditional.
fn capture_debug(f: impl FnOnce()) -> String {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(CapSource)
        .try_init();
    CAPTURE.lock().expect("capture poisoned").clear();
    f();
    CAPTURE.lock().expect("capture poisoned").clone()
}

/// Pull the `line=...` field out of one fmt-rendered event and parse it as
/// JSON: the whole point of the JSONL view is that this works.
fn wire_json(out: &str) -> serde_json::Value {
    let line = out
        .lines()
        .find(|l| l.contains("llm wire"))
        .unwrap_or_else(|| panic!("the llm wire debug event is emitted; captured [{}]", out));
    let json = line
        .split_once("llm wire line=")
        .map(|(_, rest)| rest)
        .expect("the line field carries the JSON");
    serde_json::from_str(json).expect("the line field is valid JSON")
}

#[test]
fn wire_jsonl_lines_are_parseable_and_pair_req_with_res() {
    // Positive — request line: one parseable JSON line, `raw` verbatim.
    let request = ResponseRequest::new("mock-model", None, vec![]);
    let v = wire_json(&capture_debug(|| request.wire_request_jsonl("a1b2")));
    assert_eq!(v["id"], "a1b2");
    assert_eq!(v["dir"], "req");
    assert_eq!(v["model"], "mock-model");
    let raw: serde_json::Value =
        serde_json::from_str(v["raw"].as_str().expect("raw is a string")).unwrap();
    assert_eq!(raw["model"], "mock-model");
    assert_eq!(raw["stream"], true);

    // Positive — response line: summary fields plus the verbatim raw bytes.
    let result = TurnResult {
        completed: true,
        usage: Some(Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        }),
        ..TurnResult::default()
    };
    let v = wire_json(&capture_debug(|| {
        result.wire_response_jsonl("a1b2", 42, "data: x\n");
    }));
    assert_eq!(v["id"], "a1b2");
    assert_eq!(v["dir"], "res");
    assert_eq!(v["ms"], 42);
    assert_eq!(v["completed"], true);
    assert_eq!(v["usage"]["output_tokens"], 5);
    assert_eq!(v["raw"], "data: x\n");

    // Negative — an incomplete turn without usage: `usage` is null, not a
    // fabricated object.
    let result = TurnResult::default();
    let v = wire_json(&capture_debug(|| result.wire_response_jsonl("a1b2", 7, "")));
    assert_eq!(v["usage"], serde_json::Value::Null);
    assert_eq!(v["completed"], false);
}
