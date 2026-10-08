use std::path::PathBuf;
use std::sync::Arc;

use tau_mock_llm::{scenario, server};

/// `tau-mock-llm --port N --scenarios DIR` — the deterministic
/// Responses-API SSE server the E2E mock leg and the live-* acceptance
/// suites point their provider at (phase 1 §4).
#[tokio::main]
async fn main() {
    // The per-request [route] line the e2e harness reads from stderr now routes
    // through tracing; RUST_LOG controls it, defaulting to debug so the line is
    // present. (#98)
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("tau_mock_llm=debug"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
    let mut args = std::env::args().skip(1);
    let mut port: u16 = 8123;
    let mut scenarios = PathBuf::from("fixtures/e2e-mocks");
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).expect("--port N"),
            "--scenarios" => scenarios = args.next().map(PathBuf::from).expect("--scenarios DIR"),
            other => {
                eprintln!("unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let set = Arc::new(
        scenario::ScenarioSet::load_dir(&scenarios).unwrap_or_else(|e| panic!("scenarios: {e}")),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    let addr = listener.local_addr().expect("bound listener has an addr");
    println!(
        "tau-mock-llm: listening on http://{addr} ({} scenarios from {})",
        set.len(),
        scenarios.display()
    );
    server::serve(listener, set).await;
}
