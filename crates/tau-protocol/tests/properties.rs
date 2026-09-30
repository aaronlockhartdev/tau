use proptest::prelude::*;
use tau_protocol::{Command, CommandOutput, Event};

proptest! {
    /// Parser never-panic: arbitrary text into a top-level wire type is
    /// always a Result — a panic in a parser is a bug.
    #[test]
    fn command_parse_never_panics(text in ".*") {
        let _ = serde_json::from_str::<Command>(&text);
    }

    #[test]
    fn event_parse_never_panics(text in ".*") {
        let _ = serde_json::from_str::<Event>(&text);
    }

    #[test]
    fn output_parse_never_panics(text in ".*") {
        let _ = serde_json::from_str::<CommandOutput>(&text);
    }

    /// Parsing is normalizing: a parsed value, re-serialized and re-parsed,
    /// yields a byte-identical wire form.
    #[test]
    fn command_roundtrip_is_stable(text in ".*") {
        if let Ok(cmd) = serde_json::from_str::<Command>(&text) {
            let first = serde_json::to_string(&cmd).unwrap();
            let second = serde_json::to_string(
                &serde_json::from_str::<Command>(&first).unwrap(),
            )
            .unwrap();
            prop_assert_eq!(first, second);
        }
    }

    #[test]
    fn event_roundtrip_is_stable(text in ".*") {
        if let Ok(event) = serde_json::from_str::<Event>(&text) {
            let first = serde_json::to_string(&event).unwrap();
            let second = serde_json::to_string(
                &serde_json::from_str::<Event>(&first).unwrap(),
            )
            .unwrap();
            prop_assert_eq!(first, second);
        }
    }

    #[test]
    #[test]
    fn output_roundtrip_is_stable(text in ".*") {
        if let Ok(out) = serde_json::from_str::<CommandOutput>(&text) {
            let first = serde_json::to_string(&out).unwrap();
            let second = serde_json::to_string(
                &serde_json::from_str::<CommandOutput>(&first).unwrap(),
            )
            .unwrap();
            prop_assert_eq!(first, second);
        }
    }
}
