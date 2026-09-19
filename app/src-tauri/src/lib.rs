//! tau-app: the Tauri shell. The protocol-to-core dispatch lives in
//! `core` (transport-free, spec §8/ADR-0006); Tauri itself appears only in
//! the binary's `main` (ADR-0002 library boundary).

pub mod core;

use std::sync::Arc;

use crate::core::Core;

/// Wraps the core's `Arc` so it can be managed as tauri state. rustc 1.98.1
/// computes `TypeId::of::<Arc<Core>>()` differently per instantiating crate,
/// so the type_id of the boxed value never matches the map key tauri's
/// `try_state` computes — `state()` panics at launch ("called before
/// manage()"). A non-generic struct's TypeId is stable, and everything
/// below just unwraps `.0`.
pub struct CoreState(pub Arc<Core>);
