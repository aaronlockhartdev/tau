//! The ACP (Agent Client Protocol v1) server: the headless `tau acp` mode.
//! ACP clients (Harbor, IDEs) drive a production-shape core over stdio —
//! the same dispatch protocol the app uses, no PTY scraping.

#![cfg_attr(test, allow(clippy::unwrap_used), allow(clippy::panic))]
pub mod auth;
pub mod config;
mod mapper;
pub mod pump;
pub mod sessions;
pub mod server;
pub mod transport;

/// Run the ACP server over the process stdio. Returns when stdin closes.
pub fn run() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the ACP runtime builds: a tokio builder failure has no failure mode to handle");
    runtime.block_on(server::serve(
        tokio::io::stdin(),
        std::io::stdout(),
    ));
}
