//! tau-core: all stateful tau logic as a standalone library, free of Tauri
//! types (ADR-0002) — the boundary that keeps a future `tau serve` binary a
//! transport shim (spec §2).

pub mod config;
pub mod provider;
pub mod session;
