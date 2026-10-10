//! Debounced filesystem watching (ticket #31): the shared plumbing — a
//! recursive watch set, notify's own thread, a batch channel, and stop —
//! over `notify`'s full debouncer (500 ms window; the full debouncer
//! stitches the editor's temp-write + atomic rename). The handler does a
//! non-blocking send; the consumer drains the receiver on its own thread,
//! so the only non-tokio thread is notify's own. The first consumers are
//! the skill roots (the harness); the files pane (ticket #32) is a second,
//! on the same shape but visibility-driven — it watches only the dirs the
//! pane has expanded (#115).
//!
//! Lifecycle: a watcher lives until dropped. `Debouncer::drop` only sets
//! the stop flag, so teardown goes through `stop()`, which joins the
//! thread (the design's accepted leak is the process-lifetime watcher,
//! not the test's — the test's drop joins).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use tokio::sync::mpsc;

/// The paths of one debounced batch, in the order the debouncer sorted
/// them. An empty batch is a rescan trigger (the kernel dropped events and
/// requested a rescan, or a batch error carried no paths): the consumer
/// treats it as every watched root being affected.
pub type Batch = Vec<PathBuf>;

pub struct Watcher {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    watched: HashSet<PathBuf>,
}

impl Watcher {
    /// The debouncer thread (owned by notify; idle on its tick sleep) and
    /// the app-side receiver a consumer task drains.
    pub fn new(timeout: Duration) -> (Self, mpsc::UnboundedReceiver<Batch>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let debouncer = new_debouncer(timeout, None, move |batch: DebounceEventResult| {
            let paths = match batch {
                Ok(events) => batch_paths(&events),
                Err(_) => Vec::new(),
            };
            // The channel is the batch's only failure mode: a dropped
            // receiver means the consumer is gone, and there is
            // nothing to deliver.
            let _ = tx.send(paths);
        })
        .expect("the debouncer thread spawns");
        (
            Self {
                debouncer: Some(debouncer),
                watched: HashSet::new(),
            },
            rx,
        )
    }

    /// Watch a root recursively, creating it first: both backends reject
    /// a nonexistent path, and the app already creates the roots' siblings
    /// (the `.tau/sessions` dir on session create).
    pub fn add(&mut self, root: &Path) {
        let _ = std::fs::create_dir_all(root);
        if !root.is_dir() {
            return;
        }
        if let Some(debouncer) = &mut self.debouncer
            && let Err(e) = debouncer.watch(root, RecursiveMode::Recursive)
        {
            tracing::warn!("watch {}: {e}", root.display());
        }
    }
    /// Watch a single directory non-recursively (the files pane, #115): the
    /// watcher tracks exactly the dirs the pane has expanded. Idempotent — a
    /// re-list of an already-expanded dir is a no-op, so the `FSEvents` stream
    /// is not rebuilt. A vanished dir is a no-op (nothing to watch).
    pub fn watch_path(&mut self, path: &Path) {
        if !path.is_dir() || self.watched.contains(path) {
            return;
        }
        let Some(debouncer) = &mut self.debouncer else {
            return;
        };
        if let Err(e) = debouncer.watch(path, RecursiveMode::NonRecursive) {
            tracing::warn!("watch {}: {e}", path.display());
            return;
        }
        self.watched.insert(path.to_path_buf());
    }

    /// Stop watching a directory the pane collapsed (#115). Idempotent —
    /// dropping a dir that isn't watched is a no-op.
    pub fn unwatch_path(&mut self, path: &Path) {
        if !self.watched.remove(path) {
            return;
        }
        if let Some(debouncer) = &mut self.debouncer
            && let Err(e) = debouncer.unwatch(path)
        {
            tracing::warn!("unwatch {}: {e}", path.display());
        }
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(debouncer) = self.debouncer.take() {
            debouncer.stop();
        }
    }
}

fn batch_paths(events: &[DebouncedEvent]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for event in events {
        // `DebouncedEvent` derefs to the notify `Event`; the paths are its
        // (possibly canonicalized — FSEvents does) path list.
        out.extend(event.paths.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    /// A short debounce window so the test's event arrives promptly.
    const TEST_DEBOUNCE: Duration = Duration::from_millis(50);

    /// Wait up to 5 s for a batch carrying `path` (skipping the stream's
    /// initial empty rescan batch and any unrelated batches).
    async fn wait_for_path(rx: &mut mpsc::UnboundedReceiver<Batch>, path: &Path) -> Batch {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Some(batch) = tokio::time::timeout(remaining, rx.recv())
                .await
                .ok()
                .flatten()
            else {
                panic!("no batch carrying {path:?} within 5 s");
            };
            if batch.iter().any(|p| p == path) {
                return batch;
            }
        }
    }

    #[tokio::test]
    async fn watch_path_delivers_a_change_in_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let (mut watcher, mut rx) = Watcher::new(TEST_DEBOUNCE);
        watcher.watch_path(&dir);

        let file = dir.join("a.txt");
        std::fs::write(&file, "hi").unwrap();
        wait_for_path(&mut rx, &file).await;
    }

    #[tokio::test]
    async fn unwatch_path_stops_delivery_for_that_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let (mut watcher, mut rx) = Watcher::new(TEST_DEBOUNCE);
        watcher.watch_path(&dir);

        // Prime: confirm the dir is currently watched.
        let a = dir.join("a.txt");
        std::fs::write(&a, "hi").unwrap();
        wait_for_path(&mut rx, &a).await;

        watcher.unwatch_path(&dir);
        let b = dir.join("b.txt");
        std::fs::write(&b, "hi").unwrap();
        // Quiet for 1 s: no batch may carry b.txt after the unwatch.
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(batch) = tokio::time::timeout(remaining, rx.recv())
                .await
                .ok()
                .flatten()
                && batch.iter().any(|p| p == &b)
            {
                panic!("b.txt delivered after unwatch: {batch:?}");
            }
        }
    }

    #[tokio::test]
    async fn rewatching_a_watched_dir_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let (mut watcher, _rx) = Watcher::new(TEST_DEBOUNCE);
        watcher.watch_path(&dir);
        let first = watcher.watched.len();
        watcher.watch_path(&dir);
        assert_eq!(watcher.watched.len(), first, "no duplicate watch");
    }
}
