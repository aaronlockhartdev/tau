//! The event pump: raw events → the 25 ms coalescer → a flush batch. The sink is the transport (Tauri `emit` in the app, a collector in tests), so the pipe is testable without a window (ADR-0006 transport #1).

use super::*;

pub async fn pump(core: Arc<Core>, mut sink: impl FnMut(&[Event])) {
    let mut rx = core.events();
    let mut coalescer = Coalescer::new(COALESCE_MS);
    loop {
        tokio::select! {
            received = rx.recv() => {
                let Some(event) = received else {
                    break;
                };
                let now = now_ms();
                coalescer.push(event, now);
                let batch = coalescer.take(now);
                if !batch.is_empty() {
                    sink(&batch);
                }
            }
            _ = tokio::time::sleep(deadline(&coalescer)) => {
                let batch = coalescer.take(now_ms());
                if !batch.is_empty() {
                    sink(&batch);
                }
            }
        }
    }
}

fn deadline(coalescer: &Coalescer) -> std::time::Duration {
    match coalescer.due_at() {
        Some(due) => {
            let delta = due as i128 - now_ms() as i128;
            if delta <= 0 {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_millis(delta as u64)
            }
        }
        None => std::time::Duration::from_secs(3600),
    }
}
