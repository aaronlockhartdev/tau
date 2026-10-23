//! The ACP (Agent Client Protocol v1) server: the headless `tau acp` mode.
//! ACP clients (Harbor, IDEs) drive a production-shape core over stdio —
//! the same dispatch protocol the app uses, no PTY scraping.

#![cfg_attr(test, allow(clippy::unwrap_used), allow(clippy::panic))]
pub mod auth;
pub mod config;
pub mod headless;
mod mapper;
pub mod pump;
pub mod server;
pub mod sessions;
pub mod transport;

/// Run the ACP server over the process stdio. Returns when stdin closes;
/// a runtime-build or core-build failure (an unset `HOME`) prints to
/// stderr and exits non-zero.
pub fn run() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tau-acp: tokio runtime build failed: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = runtime.block_on(server::serve(tokio::io::stdin(), std::io::stdout())) {
        eprintln!("tau-acp: {e}");
        std::process::exit(1);
    }
}
