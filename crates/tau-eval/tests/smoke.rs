//! The deterministic leg (ticket #57, §2.3): run the smoke tier against the
//! in-process mock LLM and assert harness invariants — not model quality. A
//! red here is a code problem (protocol, session integrity, tool dispatch),
//! never a network/model problem. PR-gated via the nextest job.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use tau_eval::gates;
use tau_eval::runner;
use tau_eval::task::{self, Tier};

/// The repo root: `CARGO_MANIFEST_DIR` is `crates/tau-eval`, the eval tree
/// sits at the repo root two levels up.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// The tool sequence each smoke task's known-good scenario scripts — the
/// tool-dispatch invariant the core must reproduce.
fn expected_tools(id: &str) -> Vec<&'static str> {
    match id {
        "write-file" => vec!["write"],
        "read-edit" => vec!["read", "edit"],
        "bash-create" => vec!["bash"],
        "multi-tool" => vec!["read", "edit", "bash", "write"],
        other => panic!("no expected tool sequence recorded for task {other}"),
    }
}

#[tokio::test]
async fn smoke_leg_is_green() {
    let root = repo_root();
    let all = task::load(&root.join("eval/tasks")).unwrap();
    let smoke: Vec<_> = all
        .into_iter()
        .filter(|t| t.tier() == Tier::Smoke)
        .collect();
    assert!(!smoke.is_empty(), "the smoke tier must not be empty");

    let mock = runner::start_mock(&root.join("eval/scenarios"))
        .await
        .unwrap();
    let artifacts = tempfile::tempdir().unwrap();

    let outcomes = runner::run_suite(&smoke, &mock.base_url, artifacts.path(), 4)
        .await
        .unwrap();

    assert_eq!(outcomes.len(), smoke.len(), "one outcome per smoke task");
    for o in &outcomes {
        assert_eq!(
            o.status,
            runner::Status::Pass,
            "task {} did not pass: {}",
            o.task,
            o.note
        );
        // Tool dispatch: the core invoked exactly the tools the scenario
        // scripted, in order.
        let want: Vec<String> = expected_tools(&o.task)
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            o.tool_calls, want,
            "task {} dispatched the wrong tools",
            o.task
        );
        // Session integrity: the trajectory artifact is present (its lines
        // were CRC-verified when the runner read the session back).
        let traj = artifacts.path().join(&o.task).join("session.jsonl");
        assert!(traj.is_file(), "task {} has no trajectory artifact", o.task);
    }
}

#[tokio::test]
async fn every_task_passes_the_oracle_and_nop_gates() {
    let root = repo_root();
    let all = task::load(&root.join("eval/tasks")).unwrap();
    let smoke: Vec<_> = all
        .into_iter()
        .filter(|t| t.tier() == Tier::Smoke)
        .collect();
    assert!(!smoke.is_empty(), "the smoke tier must not be empty");

    for t in &smoke {
        assert!(
            gates::oracle_passes(t).await.unwrap(),
            "task {} oracle gate failed: solve.sh did not make check.sh pass",
            t.id()
        );
        assert!(
            gates::nop_fails(t).await.unwrap(),
            "task {} nop gate failed: check.sh passed with no agent work",
            t.id()
        );
    }
}
