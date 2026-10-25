//! Background bash, session-level (#53): the completion hook wakes the
//! session (a steering-lane notification), the completion gate skips while
//! a job is live, and a session close kills live jobs.

use super::*;
use crate::agent::testkit::*;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// One observed job completion: (job id, exit code).
type HookRecord = (String, Option<i32>);

/// A session whose job-completion hook is captured by the test.
fn agent_with_hook(
    dir: &std::path::Path,
    provider: TurnProviderRef,
) -> (
    Arc<AgentSession>,
    Arc<AtomicBool>,
    Arc<Mutex<Vec<HookRecord>>>,
) {
    let agent = make_agent(dir, provider);
    let fired = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();
    let done_clone = done.clone();
    agent.set_job_notify_hook(Some(Arc::new(move |id, code, _path| {
        fired_clone.lock().unwrap().push((id.to_owned(), code));
        done_clone.store(true, Ordering::SeqCst);
    })));
    (agent, done, fired)
}

async fn poll(done: &AtomicBool, ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while !done.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the job completion hook"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_bash_returns_immediately_and_wakes_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    // Turn 1: the model detaches a bash job.
    let body1 = sse(
        "",
        &[(
            "bash".into(),
            "c1".into(),
            r#"{"command":"echo bg-done; sleep 1; exit 0","background":true}"#.into(),
        )],
    );
    let provider = Arc::new(ScriptedProvider::new(vec![body1, sse("after", &[])]));
    let (agent, done, fired) = agent_with_hook(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    // The tool entry is the immediate ack: job id + output path.
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let tool = entries.iter().find(|e| e.kind == EntryKind::Tool).unwrap();
    assert_eq!(tool.payload["name"], "bash");
    let output = tool.payload["output"].as_str().unwrap().to_owned();
    assert!(
        output.contains("job 1"),
        "the ack names the job: {output:?}"
    );
    assert!(
        output.contains(".tau/jobs/1.out"),
        "the ack names the file: {output:?}"
    );
    assert_eq!(tool.payload["status"], "ok");

    // The job's completion fires the hook (the harness turns that into the
    // wake) and clears the registry's live count.
    poll(&done, 15_000).await;
    assert!(!agent.background_jobs().has_live());
    let fired = fired.lock().unwrap();
    assert_eq!(fired[0].0, "1");
    assert_eq!(fired[0].1, Some(0));
}

#[tokio::test]
async fn the_completion_gate_skips_while_a_job_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse(
            "",
            &[(
                "bash".into(),
                "c1".into(),
                r#"{"command":"sleep 30","background":true}"#.into(),
            )],
        ),
        sse("turn over", &[]),
    ]));
    let agent = make_agent(dir.path(), provider.clone());
    // A failing gate: with a live job the gate skips (no repair bounce).
    {
        let mut inner = agent.inner.lock().unwrap();
        inner.gates = Some(crate::config::Gates {
            command: "exit 1".into(),
            ..crate::config::Gates::default()
        });
    }
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    // The gate would have bounced a repair (a failing check) — with a live
    // job it skips, so the turn stands: two assistant entries, no gate
    // record.
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let assistants = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Assistant)
        .count();
    assert_eq!(
        assistants, 2,
        "tool segment + final segment, no repair bounce"
    );
    assert!(!entries.iter().any(|e| {
        e.kind == EntryKind::System
            && e.payload
                .get("note")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|n| n.contains("completion gate"))
    }));
    // No second provider call happened: a bounced repair would have spent it.
    assert_eq!(provider.seen.lock().unwrap().len(), 2);
    agent.background_jobs().kill_all();
}

#[tokio::test]
async fn a_session_close_kills_live_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse(
        "",
        &[(
            "bash".into(),
            "c1".into(),
            r#"{"command":"sleep 30","background":true}"#.into(),
        )],
    )]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    assert!(agent.background_jobs().has_live(), "the job is live");
    agent.mark_closed();
    // kill_all aborts the waiter: the live flag is cleared immediately.
    assert!(!agent.background_jobs().has_live());
}
