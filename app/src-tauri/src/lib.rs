//! tau-app: the Tauri shell. The protocol-to-core dispatch lives in
//! `core` (transport-free, spec §8/ADR-0006); Tauri itself appears only in
//! the binary's `main` (ADR-0002 library boundary).

pub mod core;
