//! Background bash jobs (#53): a `bash` call with `background: true`
//! spawns detached, records the job in the session's registry, and returns
//! immediately with `{job_id, output_path}`. The job's output (stdout +
//! stderr) streams to `<cwd>/.tau/jobs/<jobid>.out`; at exit the code is
//! appended and the session's completion hook fires (the harness wakes the
//! session on it — the same lane as a sub-agent's report). Session close
//! kills every live job; the completion gate skips while one is live.

use crate::tools::impls::bash_path;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Fired when a background job exits: (job id, exit code — `None` when the
/// process was killed by a signal, output file path).
pub(crate) type JobCompletionHook = Arc<dyn Fn(&str, Option<i32>, &Path) + Send + Sync>;

/// One recorded job. The waiter task is the handle to the child: dropping
/// it (session close) kills the process (`kill_on_drop`).
#[allow(clippy::missing_fields_in_debug)] // `task` is not Debug
pub(crate) struct Job {
    pub(crate) id: String,
    /// The command line (the registry's record; V1 keeps it for inspection).
    #[allow(dead_code)]
    pub(crate) command: String,
    /// The output file (the registry's record; V1 keeps it for inspection).
    #[allow(dead_code)]
    pub(crate) output_path: std::path::PathBuf,
    pub(crate) live: Arc<AtomicBool>,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

/// The per-session job registry: ids are the session's per-life counter
/// ("1", "2", ...).
#[derive(Default)]
pub(crate) struct BackgroundJobs {
    next: AtomicUsize,
    jobs: Mutex<HashMap<String, Job>>,
}

impl BackgroundJobs {
    /// Record a spawned job; the id is minted here, matching the output
    /// file the spawn named it with.
    pub(crate) fn insert(&self, job: Job) {
        self.jobs
            .lock()
            .expect("jobs map: no panic while the lock is held")
            .insert(job.id.clone(), job);
    }

    /// The next job id (the spawn uses it to name the output file before
    /// the child exists).
    #[must_use]
    pub(crate) fn next_id(&self) -> String {
        (self.next.fetch_add(1, Ordering::SeqCst) + 1).to_string()
    }

    /// Whether any recorded job is still running.
    #[must_use]
    pub(crate) fn has_live(&self) -> bool {
        self.jobs
            .lock()
            .expect("jobs map: no panic while the lock is held")
            .values()
            .any(|j| j.live.load(Ordering::SeqCst))
    }

    /// Kill every live job (session close). Dropping the waiter task kills
    /// the child (`kill_on_drop`); the hook then sees the kill as a
    /// `None`-code completion.
    pub(crate) fn kill_all(&self) {
        let jobs = self
            .jobs
            .lock()
            .expect("jobs map: no panic while the lock is held");
        for j in jobs.values() {
            if j.live.load(Ordering::SeqCst) {
                j.task.abort();
                j.live.store(false, Ordering::SeqCst);
            }
        }
    }
}

/// A spawned job: the child's stdout and stderr stream to its output file
/// (the `std::process::Child` the waiter owns); at exit the waiter appends
/// the exit line, clears `live`, and fires `on_done`.
pub(crate) fn spawn(
    cwd: &Path,
    command: &str,
    jobs: &BackgroundJobs,
    on_done: Option<JobCompletionHook>,
) -> Result<(String, std::path::PathBuf), String> {
    let dir = cwd.join(".tau").join("jobs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("bash: cannot create {}: {e}", dir.display()))?;

    // The id names the output file, so mint it before spawn: a spawn
    // failure leaves a gap in the numbering, never a stale file.
    let id = jobs.next_id();
    let output_path = dir.join(format!("{id}.out"));

    // Plain argv (no stdbuf): the output lands in a file, not a stream.
    #[cfg(unix)]
    let argv = crate::tools::impls::bash_argv(command, false);
    #[cfg(not(unix))]
    let argv = vec!["bash".to_owned(), "-c".to_owned(), command.to_owned()];
    let file = File::create(&output_path)
        .map_err(|e| format!("bash: cannot open {}: {e}", output_path.display()))?;
    let child = {
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .env("PATH", bash_path())
            .current_dir(cwd)
            .stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("bash: {e}"))?
    };
    let live = Arc::new(AtomicBool::new(true));
    let task = tokio::spawn(waiter(
        child,
        live.clone(),
        id.clone(),
        output_path.clone(),
        on_done,
    ));
    jobs.insert(Job {
        id: id.clone(),
        command: command.to_owned(),
        output_path: output_path.clone(),
        live,
        task,
    });
    Ok((id, output_path))
}

/// The waiter: owns the child until it exits (a drop — session close —
/// kills it), then records the exit and fires the completion hook.
async fn waiter(
    mut child: tokio::process::Child,
    live: Arc<AtomicBool>,
    id: String,
    output_path: std::path::PathBuf,
    on_done: Option<JobCompletionHook>,
) {
    let code = child.wait().await.ok().and_then(|s| s.code());
    // Append the exit line, then clear `live` — the gate re-qualifies the
    // next turn on the wake the hook starts.
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&output_path) {
        use std::io::Write as _;
        let exit = match code {
            Some(n) => format!("exit {n}"),
            None => "exit (killed)".to_owned(),
        };
        let _ = writeln!(f, "{exit}");
    }
    live.store(false, Ordering::SeqCst);
    if let Some(hook) = on_done {
        hook(&id, code, &output_path);
    }
}

/// The model-facing result of a background spawn.
#[must_use]
pub(crate) fn spawn_output(job_id: &str, output_path: &Path) -> String {
    format!(
        "background job {job_id} started; output: {}",
        output_path.display()
    )
}
