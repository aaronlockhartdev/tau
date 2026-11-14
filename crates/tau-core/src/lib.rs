//! tau's core: one module per spec domain, named by the `owns` column in
//! `spec/index.md`. A module arrives with the change that fills it.
//!
//! Core takes no user-interface dependency (gate: core has no UI logic).
// Test code opts out of the unwrap ban, per the note in the workspace lints.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod storage;
