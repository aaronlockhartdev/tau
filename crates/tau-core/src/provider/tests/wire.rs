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
