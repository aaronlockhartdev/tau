//! The thin ACP binary: the registry's distribution artifact. The Tauri
//! app binary shares this crate for its `tau acp` branch; this one is
//! static-musl-able (no wry).

fn main() {
    tau_acp::run();
}
