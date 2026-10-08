//! The thin ACP binary: the registry's distribution artifact. The Tauri
//! app binary shares this crate for its `tau acp` branch; this one is
//! static-musl-able (no wry).

fn main() {
    // Diagnostic build only (#98): a RUST_LOG-driven subscriber routes the
    // provider wire log (and all tracing) to stdout for the trial artifact.
    // Compiled out in production builds (max_level_off → no events).
    #[cfg(feature = "log-llm-requests")]
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
    }
    tau_acp::run();
}
