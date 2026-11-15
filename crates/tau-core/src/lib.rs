//! tau's core: a module per spec domain area, named by the `owns` column in
//! `spec/index.md`. A module arrives with the change that fills it.
//!
//! Core takes no user-interface dependency (gate: core has no UI logic).
// Test code opts out of the two panic lints at the crate root (R-workspace-09).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod project;
pub mod storage;
