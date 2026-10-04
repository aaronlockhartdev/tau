//! JSON-RPC 2.0 framing over stdio (ACP v1 transport): newline-delimited
//! messages in, one locked writer out. The agent MUST NOT write anything to
//! stdout that is not a valid ACP message — every log line goes to stderr.

use std::io::Write;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// One incoming message: a request (has `id`) or a notification (none).
#[derive(Debug, Deserialize)]
pub struct Rpc {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
}

/// The single stdout writer: every ACP message serializes under this lock
/// and flushes immediately, so frames never interleave and the client sees
/// stream updates as they happen.
#[derive(Clone)]
pub struct Out {
    w: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Out {
    pub fn new(w: impl Write + Send + 'static) -> Self {
        Self {
            w: Arc::new(Mutex::new(Box::new(w))),
        }
    }

    /// Write one message as a single newline-terminated JSON line.
    pub fn send(&self, msg: &Value) {
        let line = serde_json::to_string(msg)
            .expect("ACP messages are built from json! values and always serialize");
        let mut guard = self
            .w
            .lock()
            .expect("stdout writer: no panic while the lock is held");
        let _ = writeln!(guard, "{line}");
        let _ = guard.flush();
    }
}

#[must_use]
pub fn result(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

#[must_use]
pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A `session/update` notification wrapping one update variant.
#[must_use]
pub fn session_update(session_id: &str, update: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "sessionId": session_id, "update": update },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_fields_decode() {
        let rpc: Rpc =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#)
                .unwrap();
        assert_eq!(rpc.id, Some(json!(7)));
        assert_eq!(rpc.method.as_deref(), Some("initialize"));
    }

    #[test]
    fn notification_has_no_id() {
        let rpc: Rpc =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#)
                .unwrap();
        assert!(rpc.id.is_none());
    }

    #[test]
    fn unknown_field_is_ignored() {
        // Clients may extend frames; extra fields must not break decode.
        let rpc = serde_json::from_str::<Rpc>(r#"{"jsonrpc":"2.0","id":1,"bogus":true}"#);
        assert!(rpc.is_ok(), "extra fields are ignored by serde by default");
    }

    #[test]
    fn response_shapes() {
        let id = json!(3);
        assert_eq!(
            result(&id, &json!({ "stopReason": "end_turn" })),
            json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}})
        );
        assert_eq!(
            error(&id, INVALID_PARAMS, "x"),
            json!({"jsonrpc":"2.0","id":3,"error":{"code":-32602,"message":"x"}})
        );
        let update = json!({"sessionUpdate": "agent_message_chunk"});
        assert_eq!(
            session_update("s1", &update),
            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk"}}})
        );
    }
}
