# One Tauri process with a standalone core library

The agent runs in the Tauri main process: a Rust core owns the loop, sessions, and tools, and Svelte is a thin renderer over Tauri commands/events. The core is built as a standalone library crate so future binaries (CLI, RPC) can reuse it — mirroring pi's SDK/RPC separation — but v0 ships no separate agent binary.

**Considered**: a separate agent binary that the Tauri app talks to over stdio/IPC (pi's RPC pattern). Rejected for v0: one process is simpler, and the library boundary preserves the option without the IPC cost now.
