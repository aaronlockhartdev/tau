//! The event pump: the core's event channel → the sink (Tauri `emit` in the
//! app, a collector in tests). The pipe is a plain forward — the frame
//! alignment the old 25 ms coalescer provided is the turn loop's job now
//! (ADR-0008), so nothing batches here.

use super::{Arc, Core, Event};

pub async fn pump(core: Arc<Core>, mut sink: impl FnMut(&[Event])) {
    let mut rx = core.events();
    while let Some(event) = rx.recv().await {
        sink(std::slice::from_ref(&event));
    }
}
