//! The thin ACP binary: the registry's distribution artifact. The Tauri
//! app binary shares this crate for its `tau acp` branch; this one is
//! static-musl-able (no wry).

fn main() {
    // Diagnostic build only (#98): a RUST_LOG-driven subscriber routes the
    // provider wire log (and all tracing) to stderr — never stdout, which
    // carries the ACP JSON-RPC protocol. Falls back to the wire log when
    // RUST_LOG is unset. Compiled out in production (max_level_off → no events).
    #[cfg(feature = "log-llm-requests")]
    {
        let filter = std::env::var("RUST_LOG")
            .map(tracing_subscriber::EnvFilter::new)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("tau_core=trace"));
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .try_init();
    }
    tau_acp::run();
}
