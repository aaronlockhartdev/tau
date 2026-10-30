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
    // Cap below the room the window leaves: untouched.
    assert_eq!(clamp_max_output(8192, 32768, 1000), 8192);
    // Cap above the room: clamped to `window − prompt − margin` (#338).
    assert_eq!(clamp_max_output(8192, 13_000, 1000), 7904);
    // Prompt fills the window: zero, not the wish — the gate refuses a cap of 0
    // rather than send a request with no room to answer (#338).
    assert_eq!(clamp_max_output(8192, 500, 1000), 0);
    // The undeclared-window sentinel reaches the clamp only from a turn built
    // with no config behind it; it leaves no room either.
    assert_eq!(clamp_max_output(8192, 0, 1000), 0);
}

#[test]
fn the_gate_budget_is_the_clamp_of_the_prompt_side() {
    // `prompt <= budget` is the same predicate as the clamp's
    // `prompt + cap + margin <= window` (#338): the pair is one formula.
    assert_eq!(prompt_budget(20_000, 200), 15_704);
    assert_eq!(clamp_max_output(200, 20_000, 15_704), 200);
    // A window that leaves no room once the cap and the margin are taken
    // budgets nothing.
    assert_eq!(prompt_budget(8_192, 8_192), 0);
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
    let est = prompt_token_estimate("sys", &input, "[]");
    // Bytes over four, rounded up, then the serialization allowance: the 4022-
    // byte URL is 1006, the note 1, the "sys" prompt 1, the empty tool list 1 —
    // 1009, plus a fifth for the request's JSON framing.
    assert_eq!(est, 1009 + 1009 / 5, "image and note both count");
}

#[test]
fn prompt_estimate_counts_text_call_outputs() {
    // The pre-#255 behavior for text: unchanged by the image fix.
    let input = vec![InputEntry::CallOutput(FunctionCallOutputInput {
        kind: "function_call_output",
        call_id: "c1".into(),
        output: CallOutput::Text("b".repeat(1200)),
    })];
    let est = prompt_token_estimate("sys", &input, "[]");
    // 300 for the result, 1 for the "sys" prompt, 1 for the empty tool list,
    // plus a fifth.
    assert_eq!(est, 302 + 302 / 5);
}

// --- JSONL wire view (the #98 wire log's parseable companion) ---

/// One static buffer per test binary: `set_global_default` installs once,
/// so every `llm wire` event lands here; each scenario drains it first.
static CAPTURE: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

/// A `MakeWriter` over the static buffer: the subscriber holds this unit
/// struct, and each event locks the shared buffer.
struct CapSource;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapSource {
    type Writer = CapWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        CapWriter(CAPTURE.lock())
    }
}

/// A `Write` view over one locked buffer: `MutexGuard<String>` is not
/// itself a writer.
struct CapWriter<'a>(parking_lot::MutexGuard<'a, String>);

impl std::io::Write for CapWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run `f` under a thread-scoped capture subscriber and return what it
/// emitted. Each call drains the buffer first, so a scenario only sees its
/// own event. Scoped via `set_default` rather than a global `try_init`: on
/// CI runners a global dispatch can already be installed, and a `try_init`
/// no-op would leave the buffer empty (the capture is then empty and the
/// JSONL assertions fail).
fn capture_debug(f: impl FnOnce()) -> String {
    let sub = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(CapSource)
        .finish();
    let _guard = tracing::subscriber::set_default(sub);
    CAPTURE.lock().clear();
    f();
    CAPTURE.lock().clone()
}

/// Pull the `line=...` field out of one fmt-rendered event and parse it as
/// JSON: the whole point of the JSONL view is that this works.
fn wire_json(out: &str) -> serde_json::Value {
    let line = out
        .lines()
        .find(|l| l.contains("llm wire"))
        .unwrap_or_else(|| panic!("the llm wire debug event is emitted; captured [{out}]"));
    let json = line
        .split_once("llm wire line=")
        .map(|(_, rest)| rest)
        .expect("the line field carries the JSON");
    serde_json::from_str(json).expect("the line field is valid JSON")
}

#[test]
fn wire_jsonl_lines_are_parseable_and_pair_req_with_res() {
    // Positive — request line: the delta is the *new* items only, and only
    // the tool results: the appended user message is filtered out (it is
    // static context, and a `function_call_output` is the one item type the
    // model has not seen before).
    let request = ResponseRequest::new(
        "mock-model",
        None,
        vec![
            InputEntry::Message(InputMessage {
                role: "user".into(),
                content: "old".into(),
            }),
            InputEntry::Message(InputMessage {
                role: "user".into(),
                content: "new".into(),
            }),
            InputEntry::CallOutput(FunctionCallOutputInput {
                kind: "function_call_output",
                call_id: "c1".into(),
                output: CallOutput::Text("result".into()),
            }),
        ],
    )
    .with_wire_delta(1);
    let v = wire_json(&capture_debug(|| request.wire_request_jsonl("a1b2")));
    assert_eq!(v["id"], "a1b2");
    assert_eq!(v["dir"], "req");
    assert_eq!(v["label"], serde_json::Value::Null);
    assert_eq!(v["model"], "mock-model");
    assert_eq!(v["msg_count"], 3);
    // The delta drops the appended user message (echo/static) and keeps the
    // tool result — the only genuinely-new content in the request.
    let delta = v["delta"].as_array().unwrap();
    assert_eq!(delta.len(), 1, "only the tool result is new");
    assert_eq!(delta[0]["type"], "function_call_output");
    assert_eq!(delta[0]["output"], "result");

    // Positive — a labeled OM round-trip carries its label.
    let om =
        ResponseRequest::new("mock-model", Some("observer sys"), vec![]).with_wire_label("observe");
    let v = wire_json(&capture_debug(|| om.wire_request_jsonl("c3d4")));
    assert_eq!(v["label"], "observe");
    assert_eq!(v["instructions"], "observer sys");
    assert!(v["delta"].as_array().unwrap().is_empty());

    // Positive — response line: ordered blocks plus usage, no raw bytes.
    let result = TurnResult {
        outcome: StreamOutcome::Completed,
        usage: Some(Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        }),
        blocks: vec![
            WireBlock::Reasoning {
                text: "thinking".into(),
            },
            WireBlock::ToolCall {
                text: serde_json::json!({ "name": "edit", "arguments": "{\"path\":\"a.rs\"}" })
                    .to_string(),
            },
            WireBlock::Text {
                text: "done".into(),
            },
        ],
        ..TurnResult::default()
    };
    let v = wire_json(&capture_debug(|| result.wire_response_jsonl("a1b2", 42)));
    assert_eq!(v["id"], "a1b2");
    assert_eq!(v["dir"], "res");
    assert_eq!(v["ms"], 42);
    assert_eq!(v["ended_by"], "completed");
    assert_eq!(v["usage"]["output_tokens"], 5);
    assert!(v.get("raw").is_none(), "the raw SSE bytes stay at trace");
    // The blocks preserve the model's emission order.
    let blocks = v["blocks"].as_array().unwrap();
    assert_eq!(blocks[0]["kind"], "reasoning");
    assert_eq!(blocks[1]["kind"], "tool_call");
    assert!(
        blocks[1]["text"]
            .as_str()
            .unwrap()
            .contains("\"name\":\"edit\""),
        "the merged run keeps each call's name"
    );
    assert_eq!(blocks[2]["kind"], "text");

    // Negative — an incomplete turn without usage: `usage` is null, not a
    // fabricated object.
    let result = TurnResult::default();
    let v = wire_json(&capture_debug(|| result.wire_response_jsonl("a1b2", 7)));
    assert_eq!(v["usage"], serde_json::Value::Null);
    assert_eq!(v["ended_by"], "eof_without_done");
    assert!(v["blocks"].as_array().unwrap().is_empty());
}

#[test]
fn response_wire_line_consolidates_consecutive_blocks() {
    // Positive — consecutive same-kind blocks merge into one entry, in
    // place: a kind change starts a new one.
    let result = TurnResult {
        outcome: StreamOutcome::Completed,
        blocks: vec![
            WireBlock::Reasoning { text: "hmm".into() },
            WireBlock::Reasoning {
                text: "…let me".into(),
            },
            WireBlock::Reasoning {
                text: " think".into(),
            },
            WireBlock::Text {
                text: "done".into(),
            },
            WireBlock::Text {
                text: " already".into(),
            },
            WireBlock::Reasoning {
                text: "second".into(),
            },
        ],
        ..TurnResult::default()
    };
    let blocks = result.consolidated_blocks();
    assert_eq!(blocks.len(), 3);
    assert_eq!(
        blocks[0],
        WireBlock::Reasoning {
            text: "hmm…let me think".into()
        }
    );
    assert_eq!(
        blocks[1],
        WireBlock::Text {
            text: "done already".into()
        }
    );
    assert_eq!(
        blocks[2],
        WireBlock::Reasoning {
            text: "second".into()
        }
    );

    // Positive — consecutive tool calls merge too, one JSON object per call
    // on its own line, so the run stays per-call distinct.
    let result = TurnResult {
        blocks: vec![
            WireBlock::ToolCall {
                text: r#"{"name":"read","arguments":"{\"path\":\"a.rs\"}"}"#.into(),
            },
            WireBlock::ToolCall {
                text: r#"{"name":"read","arguments":"{\"path\":\"b.rs\"}"}"#.into(),
            },
        ],
        ..TurnResult::default()
    };
    let blocks = result.consolidated_blocks();
    assert_eq!(blocks.len(), 1);
    if let WireBlock::ToolCall { text } = &blocks[0] {
        assert_eq!(text.lines().count(), 2, "each call keeps its own line");
        assert!(text.contains("\"name\":\"read\""));
    } else {
        panic!("expected a merged tool_call block");
    }

    // Negative — a single block stands alone; an empty turn is empty.
    let result = TurnResult {
        blocks: vec![WireBlock::Text {
            text: "solo".into(),
        }],
        ..TurnResult::default()
    };
    assert_eq!(
        result.consolidated_blocks(),
        vec![WireBlock::Text {
            text: "solo".into()
        }]
    );
    assert!(TurnResult::default().consolidated_blocks().is_empty());
}
