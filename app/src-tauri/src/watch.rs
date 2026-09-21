//! Debounced filesystem watching (ticket #31): the shared plumbing — a
//! recursive watch set, notify's own thread, a batch channel, and stop —
//! over `notify`'s full debouncer (500 ms window; the full debouncer
//! stitches the editor's temp-write + atomic rename). The handler does a
//! non-blocking send; the app drains the receiver on a tokio task, so the
//! only non-tokio thread is notify's own. The current consumer is the
//! skill roots (core.rs) is the first consumer; the files-pane ticket
//! (#32) is the second, on the same shape with exclusion-aware roots.
//!
//! Lifecycle: a watcher lives until dropped. `Debouncer::drop` only sets
//! the stop flag, so teardown goes through `stop()`, which joins the
//! thread (the design's accepted leak is the process-lifetime watcher,
//! not the test's — the test's drop joins).

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
            eprintln!("watch {}: {e}", root.display());
        }
    }
    /// Watch `root` with its excluded subtrees skipped (the files-pane
    /// consumer, ticket #32): the exclusion list is a hard requirement, not
    /// an optimization — on Linux inotify a recursive watch is one descriptor
    /// per directory (this repo: 4,471, 3,777 under `target/`), so the watch
    /// set is walked by hand and every non-excluded dir is added
    /// `NonRecursive`. On macOS FSEvents the stream has no per-dir
    /// descriptors, so one recursive watch suffices and the consumer
    /// post-filters events by prefix.
    pub fn add_excluded(&mut self, root: &Path, exclusions: &[&str]) {
        if !root.is_dir() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let mut stack = vec![root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                if let Some(debouncer) = &mut self.debouncer
                    && let Err(e) = debouncer.watch(&dir, RecursiveMode::NonRecursive)
                {
                    eprintln!("watch {}: {e}", dir.display());
                }
                let Ok(read) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for ent in read.flatten() {
                    let path = ent.path();
                    if path.is_dir()
                        && !exclusions.contains(&ent.file_name().to_string_lossy().as_ref())
                    {
                        stack.push(path);
                    }
                }
            }
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = exclusions;
            if let Some(debouncer) = &mut self.debouncer
                && let Err(e) = debouncer.watch(root, RecursiveMode::Recursive)
            {
                eprintln!("watch {}: {e}", root.display());
            }
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
