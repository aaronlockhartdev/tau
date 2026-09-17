//! The core↔GUI message boundary (spec §8, ADR-0006): one crate of tagged-union
//! messages — commands in, events out — JSON-serializable, free of Tauri types.
//! Tauri commands/events are transport #1; a future `tau serve` carries the
//! same types over a network transport. The full set is finalized by the
//! protocol milestone (map #21); this skeleton pins the envelope shape.

use serde::{Deserialize, Serialize};

/// A command from any transport client to the core.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    WorkspaceOpen { cwd: String },
    WorkspaceList,
    SessionList { workspace: String },
    ProviderList,
}

/// An event pushed from the core to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SystemError { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrips_through_json() {
        let cmd = Command::WorkspaceOpen {
            cwd: "/tmp/demo".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"type":"workspace_open","cwd":"/tmp/demo"}"#);
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::WorkspaceOpen { cwd } => assert_eq!(cwd, "/tmp/demo"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn event_roundtrips_through_json() {
        let ev = Event::SystemError {
            message: "boom".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"type":"system_error","message":"boom"}"#);
        assert!(serde_json::from_str::<Event>(&json).is_ok());
    }
}
