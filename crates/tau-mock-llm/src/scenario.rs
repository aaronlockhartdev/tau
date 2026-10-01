use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};

/// One scripted turn: a streamed text, a batch of tool calls, or both.
#[derive(Debug, Clone, Deserialize)]
pub struct Turn {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub calls: Vec<Call>,
}

/// One tool call; `arguments` is a full JSON object in the file and is
/// serialized to the complete-JSON-string wire shape at render time.
#[derive(Debug, Clone, Deserialize)]
pub struct Call {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            input_tokens: 120,
            output_tokens: 48,
            total_tokens: 168,
        }
    }
}

/// A scenario: a content match plus a sequential turn list.
#[derive(Debug, Deserialize)]
pub struct Scenario {
    #[serde(rename = "match")]
    pub pattern: String,
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(skip)]
    counter: AtomicUsize,
}

impl Scenario {
    pub fn new(pattern: impl Into<String>, turns: Vec<Turn>, usage: Usage) -> Self {
        Self {
            pattern: pattern.into(),
            turns,
            usage,
            counter: AtomicUsize::new(0),
        }
    }

    /// Next turn, sequential per scenario; a request that lands past the
    /// end of the script clamps to the final turn (a stuck client repeats
    /// the ending instead of hanging).
    pub fn next_turn(&self) -> (usize, &Turn) {
        // load_dir rejects an empty script; the guard is for direct `new`
        // callers (an empty `turns` would underflow the clamp).
        assert!(!self.turns.is_empty(), "a scenario needs at least one turn");
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let i = n.min(self.turns.len() - 1);
        (i, &self.turns[i])
    }
}

/// The request fields selection reads: the system instructions and the
/// input items (messages carry `role`/`content`; the call/output item
/// shapes are irrelevant to matching).
#[derive(Debug, Default, Deserialize)]
pub struct WireRequest {
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub input: Vec<Value>,
}

/// Built-in answer for a request no scenario claims: short, terminated,
/// deterministic — a spec can never hang the stream.
pub fn fallback() -> Scenario {
    Scenario {
        pattern: String::new(),
        turns: vec![Turn {
            text: Some(
                "mock fallback: no scenario matched this request; the turn ends here.".into(),
            ),
            calls: vec![],
        }],
        usage: Usage::default(),
        counter: AtomicUsize::new(0),
    }
}

/// All scenarios of one `--scenarios` directory, in file-name order (the
/// deterministic match priority).
pub struct ScenarioSet {
    pub scenarios: Vec<Scenario>,
    fallback: Scenario,
}

impl Default for ScenarioSet {
    fn default() -> Self {
        Self {
            scenarios: Vec::new(),
            fallback: fallback(),
        }
    }
}

impl ScenarioSet {
    pub fn load_dir(dir: &Path) -> Result<Self, String> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("scenarios dir {}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        let mut scenarios = Vec::new();
        for path in paths {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let mut scenario: Scenario =
                serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
            if scenario.turns.is_empty() {
                return Err(format!(
                    "{}: a scenario needs at least one turn",
                    path.display()
                ));
            }
            // The counter is not deserializable; start it fresh.
            scenario.counter = AtomicUsize::new(0);
            scenarios.push(scenario);
        }
        Ok(ScenarioSet {
            scenarios,
            fallback: fallback(),
        })
    }

    pub const fn len(&self) -> usize {
        self.scenarios.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.scenarios.is_empty()
    }

    /// The scenario a request belongs to: the `instructions` text first
    /// (the observer/reflector live there), then the user messages in
    /// order; the fallback when nothing matches.
    pub fn select(&self, req: &WireRequest) -> &Scenario {
        if let Some(ins) = &req.instructions
            && let Some(hit) = self.scenarios.iter().find(|s| ins.contains(&s.pattern))
        {
            return hit;
        }
        for item in &req.input {
            let Some(obj) = item.as_object() else {
                continue;
            };
            if obj.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            let Some(content) = obj.get("content").and_then(Value::as_str) else {
                continue;
            };
            if let Some(hit) = self.scenarios.iter().find(|s| content.contains(&s.pattern)) {
                return hit;
            }
        }
        &self.fallback
    }
}

/// The `data:` payloads of one turn, in wire order: `response.created`,
/// the text split into deltas, one complete `response.output_item.done`
/// per function call, `response.completed` with usage, `[DONE]`.
pub fn render_frames(turn: &Turn, usage: &Usage, seq: usize) -> Vec<String> {
    let id = format!("mock-resp-{seq}");
    let mut frames = vec![
        json!({
            "type": "response.created",
            "response": { "id": id }
        })
        .to_string(),
    ];
    if let Some(text) = &turn.text {
        for chunk in chunk_text(text) {
            frames
                .push(json!({ "type": "response.output_text.delta", "delta": chunk }).to_string());
        }
    }
    for (i, call) in turn.calls.iter().enumerate() {
        frames.push(
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "id": format!("mock-item-{seq}-{i}"),
                    "call_id": format!("mock-call-{seq}-{i}"),
                    "name": call.name,
                    "arguments": call.arguments.to_string(),
                }
            })
            .to_string(),
        );
    }
    frames.push(
        json!({
            "type": "response.completed",
            "response": { "id": id, "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "total_tokens": usage.total_tokens,
            } }
        })
        .to_string(),
    );
    frames.push("[DONE]".into());
    frames
}

/// Char-safe fixed-width chunks: the deltas concatenate back to the exact
/// source text (no word-boundary assumptions).
fn chunk_text(text: &str) -> Vec<String> {
    const WIDTH: usize = 32;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= WIDTH {
        return vec![text.to_owned()];
    }
    chars.chunks(WIDTH).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_round_trips() {
        for text in [
            "short".to_string(),
            "x".repeat(32),
            "w ".repeat(50),
            "héllo wörld — char safe".to_string(),
        ] {
            let chunks = chunk_text(&text);
            let joined: String = chunks.concat();
            assert_eq!(joined, text, "chunks must concatenate exactly");
        }
    }

    #[test]
    fn a_call_turn_renders_the_complete_arguments_string() {
        let turn = Turn {
            text: None,
            calls: vec![Call {
                name: "write".into(),
                arguments: json!({ "path": "a.txt", "content": "hi" }),
            }],
        };
        let frames = render_frames(&turn, &Usage::default(), 0);
        let done = frames
            .iter()
            .find(|f| f.contains("\"type\":\"response.output_item.done\""))
            .expect("the output_item.done frame");
        let v: Value = serde_json::from_str(done).unwrap();
        let item = &v["item"];
        assert_eq!(item["name"], "write");
        // The wire contract: arguments is a JSON *string*, whole in one
        // event (the client does not reassemble argument fragments).
        assert!(item["arguments"].is_string());
        assert_eq!(
            serde_json::from_str::<Value>(item["arguments"].as_str().unwrap()).unwrap(),
            json!({ "path": "a.txt", "content": "hi" })
        );
        assert_eq!(frames.last().unwrap(), "[DONE]");
    }

    #[test]
    fn selection_prefers_instructions_over_messages() {
        let mut set = ScenarioSet::default();
        set.scenarios.push(Scenario {
            pattern: "observer-marker".into(),
            turns: vec![Turn {
                text: Some("obs".into()),
                calls: vec![],
            }],
            usage: Usage::default(),
            counter: AtomicUsize::new(0),
        });
        let req = WireRequest {
            instructions: Some("you are the X observer-marker Y".into()),
            input: vec![json!({ "role": "user", "content": "body" })],
        };
        assert_eq!(set.select(&req).pattern, "observer-marker");
        // No marker anywhere: the fallback, not an error.
        let plain = WireRequest {
            instructions: None,
            input: vec![json!({ "role": "user", "content": "hello" })],
        };
        assert_eq!(set.select(&plain).pattern, "");
    }

    #[test]
    fn the_turn_counter_clamps_at_the_end_of_the_script() {
        let s = Scenario {
            pattern: "x".into(),
            turns: vec![
                Turn {
                    text: Some("one".into()),
                    calls: vec![],
                },
                Turn {
                    text: Some("two".into()),
                    calls: vec![],
                },
            ],
            usage: Usage::default(),
            counter: AtomicUsize::new(0),
        };
        assert_eq!(s.next_turn().1.text.as_deref(), Some("one"));
        assert_eq!(s.next_turn().1.text.as_deref(), Some("two"));
        assert_eq!(s.next_turn().1.text.as_deref(), Some("two"));
    }
}
