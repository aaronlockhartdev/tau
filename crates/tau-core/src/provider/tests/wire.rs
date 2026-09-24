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
}

