//! Background bash job tests (#53): spawn/registry semantics (positive) and
//! spawn failure (negative).

use super::bgjobs::{BackgroundJobs, JobCompletionHook};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[tokio::test]
async fn a_background_job_streams_output_and_fires_the_hook() {
    let d = dir();
    let jobs = BackgroundJobs::default();
    let done = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(Mutex::new(Vec::new()));
    let done_clone = done.clone();
    let fired_clone = fired.clone();
    let hook: JobCompletionHook = Arc::new(move |id, code, path| {
        fired_clone
            .lock()
            .push((id.to_owned(), code, path.to_path_buf()));
        done_clone.store(true, Ordering::SeqCst);
    });

    let (id, path) = super::bgjobs::spawn(
        d.path(),
        "echo hello-bg; echo err 1>&2; exit 0",
        &jobs,
        Some(hook),
    )
    .unwrap();
    // The spawn is recorded and live before the hook can fire.
    assert_eq!(id, "1");
    assert!(jobs.has_live(), "the job is live after spawn");
    assert!(
        path.starts_with(d.path()),
        "the output file is under the session cwd: {path:?}"
    );

    // The command is fast: poll the hook for a bounded time.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the completion hook never fired"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!jobs.has_live(), "the job is not live after exit");
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains("hello-bg"), "stdout landed: {file:?}");
    assert!(file.contains("err"), "stderr landed: {file:?}");
    assert!(
        file.contains("exit 0"),
        "the exit line was appended: {file:?}"
    );
    let fired = fired.lock();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].0, "1");
    assert_eq!(fired[0].1, Some(0));
}

#[tokio::test]
async fn a_failing_job_carries_its_exit_code() {
    let d = dir();
    let jobs = BackgroundJobs::default();
    let (codes, done) = (
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(AtomicBool::new(false)),
    );
    let codes_clone = codes.clone();
    let done_clone = done.clone();
    let hook: JobCompletionHook = Arc::new(move |_id, code, _path| {
        codes_clone.lock().push(code);
        done_clone.store(true, Ordering::SeqCst);
    });
    super::bgjobs::spawn(d.path(), "echo fail; exit 3", &jobs, Some(hook)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the completion hook never fired"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(*codes.lock(), vec![Some(3)]);
}

#[tokio::test]
async fn job_ids_are_per_session_sequential() {
    let d = dir();
    let jobs = BackgroundJobs::default();
    let a = super::bgjobs::spawn(d.path(), "true", &jobs, None).unwrap();
    let b = super::bgjobs::spawn(d.path(), "true", &jobs, None).unwrap();
    assert_eq!(a.0, "1");
    assert_eq!(b.0, "2");
    // The registry's live count reflects both until they exit.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while jobs.has_live() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!jobs.has_live());
}

#[test]
fn a_spawn_failure_is_an_error_not_a_recorded_job() {
    let d = dir();
    let jobs = BackgroundJobs::default();
    // A cwd that is a file: `create_dir_all` on the jobs dir under it must
    // fail, so the spawn is an error and nothing is recorded.
    let cwd = d.path().join("a-file");
    std::fs::write(&cwd, "x").unwrap();
    let r = super::bgjobs::spawn(&cwd, "true", &jobs, None);
    assert!(r.is_err(), "expected a spawn error, got {r:?}");
    assert!(!jobs.has_live());
}
