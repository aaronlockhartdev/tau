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
        kind: ToolKind::Function,
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
        output: CallOutput::Image(vec![InputImagePart {
            kind: "input_image",
            image_url: "data:image/png;base64,AAAA".into(),
        }]),
    })
    .unwrap();
    assert_eq!(image["output"][0]["type"], "input_image");
    assert_eq!(
        image["output"][0]["image_url"],
        "data:image/png;base64,AAAA"
    );
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
        Some(CallOutput::Image(vec![InputImagePart {
            kind: "input_image",
            image_url: "data:image/png;base64,AAAA".into(),
        }]))
    );
    assert_eq!(
        CallOutput::from_payload(&serde_json::json!("plain")),
        Some(CallOutput::Text("plain".into()))
    );
    assert_eq!(
        CallOutput::from_payload(&serde_json::json!({"other": 1})),
        None
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
