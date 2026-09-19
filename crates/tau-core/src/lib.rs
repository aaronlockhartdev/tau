//! tau-core: all stateful tau logic as a standalone library, free of Tauri
//! types (ADR-0002) — the boundary that keeps a future `tau serve` binary a
//! transport shim (spec §2).

pub mod agent;
pub mod agent_type;
pub mod config;
pub mod context;
pub mod hashline;
pub mod om;
pub mod om_integration;
pub mod provider;
pub mod session;
pub mod subagent;
pub mod task;
pub mod tools;
