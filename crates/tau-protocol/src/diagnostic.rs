use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How a provider stream ended (#306, ADR-0034). The bool it replaces could
/// not tell a cutoff — a turn cut by the provider — from a clean stop, so a
/// stopped agent left no fact behind.
///
/// The wire spellings are append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEnd {
    /// The terminal frame arrived.
    Completed,
    /// The provider stopped at its output cap.
    Length,
    /// The stream went quiet past the idle deadline.
    IdleTimeout,
    /// The body ended with no terminal frame.
    EofWithoutDone,
    /// An error arrived mid-stream.
    MidStreamError,
    /// The user cut the turn (force-send).
    Killed,
}

/// A fact the harness records about a session (ADR-0034). It lives in the
/// entry log under kind `diagnostic`; nothing may exist only as a log line
/// if the harness, the GUI, or a reader of a closed session needs it.
///
/// The `code` spellings are append-only: never rename or remove one,
/// deprecate it instead. There is no display string — the GUI composes the
/// sentence from the code and its fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Diagnostic {
    /// The effective model changed. `from` is absent when the session never
    /// recorded a model — a config-level change, the case that left the
    /// incident undatable.
    ModelChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        to: String,
    },
    /// The context gate could not be armed: a named model fact is undeclared,
    /// so nothing bounds the prompt against the model's window.
    GateUnavailable { model: String, missing: Vec<String> },
    /// A request was refused because the prompt passed the gate budget (#255).
    ContextOverflow { prompt: u64, budget: u64 },
    /// The completion gate's check failed and the model was bounced (the
    /// record names the command: config changes, history must not).
    GateAttempt {
        command: String,
        retries: u64,
        max: u64,
        output: String,
    },
    /// The completion gate's repairs are exhausted; the session is marked
    /// `gate_failed`. The per-attempt output lives in the attempt records.
    GateFailed { command: String, retries: u64 },
    /// OM's mid-loop buffering went dark: pending input reached the
    /// activation threshold with no compaction route left in the turn.
    OmStalled { pending: u64, activation: u64 },
    /// The provider answered a request with an error status.
    ProviderRejected { status: u16, body: String },
    /// A provider stream ended without a completion signal.
    TurnCutoff {
        model: String,
        ended_by: StreamEnd,
        blocks: u64,
    },
    /// A code this build does not know, kept verbatim so the line still
    /// loads and renders by name. The entry's raw payload — not this
    /// variant — is what the GUI reads, so an unknown code is never written
    /// back; it exists to be read, not re-emitted.
    Unknown { unknown_code: String },
}

impl Diagnostic {
    /// The code as it appears on the wire.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::ModelChanged { .. } => "model_changed",
            Self::GateUnavailable { .. } => "gate_unavailable",
            Self::ContextOverflow { .. } => "context_overflow",
            Self::GateAttempt { .. } => "gate_attempt",
            Self::GateFailed { .. } => "gate_failed",
            Self::OmStalled { .. } => "om_stalled",
            Self::ProviderRejected { .. } => "provider_rejected",
            Self::TurnCutoff { .. } => "turn_cutoff",
            Self::Unknown { unknown_code } => unknown_code,
        }
    }

    /// The entry payload: the tagged map this enum serializes to.
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a Diagnostic always serializes")
    }

    /// Read a diagnostic entry's payload back. None when the payload is not
    /// a diagnostic map at all; an unknown `code` becomes [`Diagnostic::Unknown`].
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

// Hand-written rather than derived: `#[serde(other)]` covers unit variants
// only, and an unknown code must keep its name for the reader (ADR-0034's
// forward compatibility), which the derive would drop. A payload with no
// `code` key is not a diagnostic at all, hence `from_value`'s None.
impl<'de> Deserialize<'de> for Diagnostic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let code = match value.get("code").and_then(Value::as_str) {
            Some(code) => code.to_owned(),
            None => return Err(D::Error::custom("diagnostic payload has no code")),
        };
        let string = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let opt_string = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| value.get(key).and_then(Value::as_u64);
        Ok(match code.as_str() {
            "model_changed" => Self::ModelChanged {
                from: opt_string("from"),
                to: string("to"),
            },
            "gate_unavailable" => Self::GateUnavailable {
                model: string("model"),
                missing: value
                    .get("missing")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "context_overflow" => Self::ContextOverflow {
                prompt: number("prompt").unwrap_or_default(),
                budget: number("budget").unwrap_or_default(),
            },
            "gate_attempt" => Self::GateAttempt {
                command: string("command"),
                retries: number("retries").unwrap_or_default(),
                max: number("max").unwrap_or_default(),
                output: string("output"),
            },
            "gate_failed" => Self::GateFailed {
                command: string("command"),
                retries: number("retries").unwrap_or_default(),
            },
            "om_stalled" => Self::OmStalled {
                pending: number("pending").unwrap_or_default(),
                activation: number("activation").unwrap_or_default(),
            },
            "provider_rejected" => Self::ProviderRejected {
                status: value
                    .get("status")
                    .and_then(Value::as_u64)
                    .and_then(|v| u16::try_from(v).ok())
                    .unwrap_or_default(),
                body: string("body"),
            },
            "turn_cutoff" => Self::TurnCutoff {
                model: string("model"),
                ended_by: value
                    .get("ended_by")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or(StreamEnd::EofWithoutDone),
                blocks: number("blocks").unwrap_or_default(),
            },
            other => Self::Unknown {
                unknown_code: other.to_owned(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(d: &Diagnostic) -> Diagnostic {
        let value = serde_json::to_value(d).expect("serializes");
        serde_json::from_value(value).expect("deserializes")
    }

    #[test]
    fn every_known_code_round_trips_with_its_fields() {
        let cases = vec![
            Diagnostic::ModelChanged {
                from: Some("a".into()),
                to: "b".into(),
            },
            Diagnostic::ModelChanged {
                from: None,
                to: "b".into(),
            },
            Diagnostic::GateUnavailable {
                model: "b".into(),
                missing: vec!["context_window".into()],
            },
            Diagnostic::ContextOverflow {
                prompt: 120_000,
                budget: 110_592,
            },
            Diagnostic::GateAttempt {
                command: "cargo test".into(),
                retries: 1,
                max: 2,
                output: "test failed".into(),
            },
            Diagnostic::GateFailed {
                command: "cargo test".into(),
                retries: 2,
            },
            Diagnostic::OmStalled {
                pending: 24_000,
                activation: 24_000,
            },
            Diagnostic::ProviderRejected {
                status: 400,
                body: "context length".into(),
            },
            Diagnostic::TurnCutoff {
                model: "b".into(),
                ended_by: StreamEnd::EofWithoutDone,
                blocks: 1,
            },
        ];
        for case in cases {
            assert_eq!(round_trip(&case), case, "code {}", case.code());
        }
    }

    #[test]
    fn the_payload_is_tagged_by_code() {
        let value = Diagnostic::ContextOverflow {
            prompt: 5,
            budget: 4,
        }
        .to_value();
        assert_eq!(value["code"], "context_overflow");
        assert_eq!(value["prompt"], 5);
    }

    // A config-level change has no recorded predecessor: the record says
    // which model took over and stays silent about where it came from,
    // rather than inventing an empty one.
    #[test]
    fn a_model_change_without_a_predecessor_records_no_from() {
        let value = Diagnostic::ModelChanged {
            from: None,
            to: "b".into(),
        }
        .to_value();
        assert_eq!(value["code"], "model_changed");
        assert_eq!(value["to"], "b");
        assert!(value.get("from").is_none(), "got: {value}");
    }

    #[test]
    fn an_unknown_code_keeps_its_name_and_the_line_still_loads() {
        let value = serde_json::json!({ "code": "future_thing", "detail": "kept" });
        let parsed = Diagnostic::from_value(&value).expect("a payload with a code parses");
        assert_eq!(
            parsed,
            Diagnostic::Unknown {
                unknown_code: "future_thing".into()
            }
        );
        assert_eq!(parsed.code(), "future_thing");
    }

    // `code()` feeds the log line and the list preview, so it must agree with
    // the tag the wire carries rather than be a second hand-maintained list.
    #[test]
    fn code_is_the_wire_spelling_for_every_known_code() {
        let facts = [
            Diagnostic::ModelChanged {
                from: None,
                to: "m".into(),
            },
            Diagnostic::GateUnavailable {
                model: "m".into(),
                missing: vec!["context_window".into()],
            },
            Diagnostic::ContextOverflow {
                prompt: 9,
                budget: 8,
            },
            Diagnostic::GateAttempt {
                command: "c".into(),
                retries: 1,
                max: 2,
                output: String::new(),
            },
            Diagnostic::GateFailed {
                command: "c".into(),
                retries: 2,
            },
            Diagnostic::OmStalled {
                pending: 9,
                activation: 8,
            },
            Diagnostic::ProviderRejected {
                status: 400,
                body: "no".into(),
            },
            Diagnostic::TurnCutoff {
                model: "m".into(),
                ended_by: StreamEnd::Length,
                blocks: 3,
            },
        ];
        for fact in facts {
            let tag = serde_json::to_value(&fact).unwrap()["code"]
                .as_str()
                .expect("the internal tag")
                .to_owned();
            assert_eq!(fact.code(), tag, "{fact:?}");
        }
    }

    #[test]
    fn a_payload_with_no_code_is_not_a_diagnostic() {
        assert_eq!(
            Diagnostic::from_value(&serde_json::json!({ "note": "hi" })),
            None
        );
    }

    #[test]
    fn the_stream_end_spellings_are_pinned() {
        assert_eq!(
            serde_json::to_value(StreamEnd::IdleTimeout).unwrap(),
            "idle_timeout"
        );
        assert_eq!(serde_json::to_value(StreamEnd::Killed).unwrap(), "killed");
    }
}
