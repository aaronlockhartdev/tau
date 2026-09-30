//! Deterministic mock LLM for the E2E and acceptance suites (phase 1 §4):
//! serves the OpenAI Responses API SSE dialect that `tau-core`'s provider
//! decodes (spec §6) from committed scenario files. One server hosts every
//! flow (parent, child, observer, reflector, plain turns) because the
//! scenario is chosen by request content, not by connection.

pub mod scenario;
pub mod server;
