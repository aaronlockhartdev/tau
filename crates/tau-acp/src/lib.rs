//! The ACP (Agent Client Protocol v1) server: the headless `tau acp` mode.
//! ACP clients (Harbor, IDEs) drive a production-shape core over stdio —
//! the same dispatch protocol the app uses, no PTY scraping.

#![cfg_attr(test, allow(clippy::unwrap_used), allow(clippy::panic))]
pub mod auth;
pub mod config;
mod mapper;
pub mod pump;
pub mod sessions;
pub mod transport;
