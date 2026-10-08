//! The thin ACP binary: the registry's distribution artifact. The Tauri
//! app binary shares this crate for its `tau acp` branch; this one is
//! static-musl-able (no wry).

fn main() {
    // A RUST_LOG-driven subscriber routes the provider wire log (and all
    // tracing) to stderr — never stdout, which carries the ACP JSON-RPC
    // protocol. Falls back to the wire log when RUST_LOG is unset. In a
    // production build (max_level_off) the trace! emitters are compiled out, so
    // this subscriber simply receives no events.
    let filter = std::env::var("RUST_LOG").map_or_else(
        |_| tracing_subscriber::EnvFilter::new("tau_core=trace"),
        tracing_subscriber::EnvFilter::new,
    );
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
    tau_acp::run();
}
