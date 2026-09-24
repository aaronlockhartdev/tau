use super::*;

use crate::agent::testkit::*;
use crate::provider::canned_cut;

#[tokio::test]
async fn task_tools_run_through_the_loop_and_the_gate_enforces_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = session_in(dir.path());
    // The session gets the full non-child tool set (incl. tasks).
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::agent_tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: Arc::new(ScriptedProvider::new(vec![
            sse(
                "",
                &[(
                    "task_create".into(),
                    "c1".into(),
                    r#"{"title":"write the docs","criteria":["docs exist"]}"#.into(),
                )],
            ),
            sse(
                "",
                &[(
                    "task_start".into(),
                    "c2".into(),
                    r#"{"task":"task-1"}"#.into(),
                )],
            ),
            // The gate: no evidence yet → the finish fails with the gap.
            sse(
                "",
                &[(
                    "task_finish".into(),
                    "c3".into(),
                    r#"{"task":"task-1"}"#.into(),
                )],
            ),
            sse(
                "",
                &[(
                    "task_evidence".into(),
                    "c4".into(),
                    r#"{"task":"task-1","criterion":"docs exist","summary":"they do"}"#.into(),
                )],
            ),
            // The same finish now passes.
            sse(
                "",
                &[(
                    "task_finish".into(),
                    "c5".into(),
                    r#"{"task":"task-1"}"#.into(),
                )],
            ),
            sse("done", &[]),
        ])),
        tool_batch_on_force: Default::default(),
        turn: Default::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("work the task", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let out = |id: &str| -> String {
        entries
            .iter()
            .find(|e| e.payload["call_id"] == id)
            .unwrap()
            .payload["output"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(out("c1").contains("created task-1"), "{}", out("c1"));
    assert!(out("c2").contains("[in_progress]"), "{}", out("c2"));
    assert!(
        out("c3").contains("completion gate failed"),
        "{}",
        out("c3")
    );
    assert!(out("c4").contains("[in_progress]"), "{}", out("c4"));
    assert!(out("c5").contains("[done]"), "{}", out("c5"));
    // The session file holds the task events, and the fold ends done.
    let task_entries = entries
        .iter()
        .filter(|e| e.kind == crate::task::KIND_TASK)
        .count();
    assert_eq!(task_entries, 4); // the failed finish appends nothing
    let tasks = crate::task::fold_entries(&entries);
    assert_eq!(tasks[0].status, crate::task::STATUS_DONE);
}

#[tokio::test]
async fn a_plain_turn_appends_user_then_assistant() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    let agent = make_agent(dir.path(), provider);
    agent.send("hello", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    assert_eq!(
        entries.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        vec![KIND_USER, KIND_ASSISTANT]
    );
    assert_eq!(entries[0].payload["text"], "hello");
    assert_eq!(entries[1].payload["text"], "done");
    assert!(!entries[1].payload["interrupted"].as_bool().unwrap());
}

#[tokio::test]
async fn tool_calls_roundtrip_through_the_session() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let body1 = sse(
        "",
        &[("read".into(), "c1".into(), r#"{"path":"n.txt"}"#.into())],
    );
    let body2 = sse("read it", &[]);
    let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
    let agent = make_agent(dir.path(), provider);
    agent.send("read n.txt", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![KIND_USER, KIND_ASSISTANT, KIND_TOOL, KIND_ASSISTANT]
    );
    assert_eq!(entries[2].payload["call_id"], "c1");
    assert_eq!(entries[2].payload["name"], "read");
    assert!(entries[2].payload["output"].as_str().unwrap().contains("a"));
}

#[tokio::test]
async fn steering_lands_on_the_next_llm_call() {
    let dir = tempfile::tempdir().unwrap();
    let body1 = sse(
        "",
        &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
    );
    let body2 = sse("after steering", &[]);
    let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
    let agent = make_agent(dir.path(), provider);
    agent.send("start", Lane::FollowUp);
    agent.send("steer me", Lane::Steering);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    // The steering message rides the next LLM call: it is a user entry
    // delivered at the head of the same turn as its starter.
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            KIND_USER,
            KIND_USER,
            KIND_ASSISTANT,
            KIND_TOOL,
            KIND_ASSISTANT
        ]
    );
    assert_eq!(entries[1].payload["text"], "steer me");
    assert_eq!(entries[1].payload["lane"], "steering");
}

#[tokio::test]
async fn follow_up_lands_after_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("first done", &[]),
        sse("second done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("first", Lane::FollowUp);
    agent.send("later", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![KIND_USER, KIND_ASSISTANT, KIND_USER, KIND_ASSISTANT]
    );
    assert_eq!(entries[1].payload["text"], "first done");
    // The follow-up starts the *next* turn: its user entry sits between
    // the two assistant entries, not inside the first turn.
    assert_eq!(entries[2].payload["text"], "later");
    assert_eq!(entries[3].payload["text"], "second done");
}

#[tokio::test]
async fn force_kills_the_stream_and_keeps_the_partial() {
    let dir = tempfile::tempdir().unwrap();
    // A long stream the force cuts after two text events; no calls, so
    // the turn ends at the kill.
    let body = sse("a", &[]) + sse("b", &[]).as_str();
    let provider = canned_cut(&body, 1);
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let assistant = entries.iter().find(|e| e.kind == KIND_ASSISTANT).unwrap();
    assert!(assistant.payload["interrupted"].as_bool().unwrap());
    assert_eq!(assistant.payload["text"], "a");
}

#[tokio::test]
async fn force_preempts_a_queued_steering_at_the_head_of_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("killed", &[]),
        sse("next", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.send("queued steering", Lane::Steering);
    // The force arrives mid-turn: kill + head of queue.
    agent.send("FORCE", Lane::Force);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let users: Vec<&str> = entries
        .iter()
        .filter(|e| e.kind == KIND_USER)
        .map(|e| e.payload["text"].as_str().unwrap())
        .collect();
    // The forced message is delivered before the queued steering, on
    // the turn that follows the killed one.
    assert_eq!(users, vec!["FORCE", "queued steering", "go"]);
}

/// A force sent while the stream is IN FLIGHT (not a pre-cut): the kill
/// flag stops the slow stream mid-way, the partial stands as interrupted,
/// and the forced message preempts a steering queued at the same instant
/// on the next call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_mid_stream_kills_the_stream_and_preempts_contemporaneous_steering() {
    let dir = tempfile::tempdir().unwrap();
    // One stream, four 40 ms-apart text deltas, ending in a single
    // completed event: left alone it runs ~200 ms to completion, the
    // force at ~100 ms cuts it mid-way. (Built by hand — sse() ends each
    // chunk in [DONE], and the decoder stops at the first one.)
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"c\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"d\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        "data: [DONE]\n\n"
    );
    let provider = crate::provider::canned_slow(body, 40);
    let agent = Arc::new(make_agent(dir.path(), provider));
    agent.send("go", Lane::FollowUp);
    let killer = {
        let agent = agent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            agent.send("FORCE", Lane::Force);
            agent.send("steer", Lane::Steering);
        })
    };
    agent.process().await.unwrap();
    killer.await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            KIND_USER,
            KIND_ASSISTANT,
            KIND_USER,
            KIND_USER,
            KIND_ASSISTANT
        ]
    );
    let killed = &entries[1];
    assert!(
        killed.payload["interrupted"].as_bool().unwrap(),
        "{killed:?}"
    );
    // a, b, c land before the 100 ms force (d is at 120 ms); under a
    // loaded runner the 80 ms c may lose the race, so accept ab or abc.
    let partial = killed.payload["text"].as_str().unwrap();
    assert!(
        partial == "ab" || partial == "abc",
        "partial was {partial:?}"
    );
    assert_eq!(entries[2].payload["text"], "FORCE");
    assert_eq!(entries[3].payload["text"], "steer");
    // The following call starts un-killed and runs the stream to completion.
    assert_eq!(entries[4].payload["text"], "abcd");
    assert!(!entries[4].payload["interrupted"].as_bool().unwrap());
}

/// Live acceptance (ticket #19): a four-tool session against the hosted
/// vLLM endpoint; skipped unless TAU_TEST_ENDPOINT is set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_tool_calling_session() {
    let base = match std::env::var("TAU_TEST_ENDPOINT") {
        Ok(base) => base,
        Err(_) => return,
    };
    let model = std::env::var("TAU_TEST_MODEL").unwrap_or_else(|_| "qwen3.8-27b".into());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "line1\nline2\nline3\n").unwrap();
    let provider = crate::provider::production(
        &crate::provider::tests::test_client(),
        &crate::config::Provider {
            base_url: base,
            key_env: String::new(),
            models: vec![model.clone()],
        },
        &crate::config::Requests::default(),
    );
    let agent = AgentSession::new(SessionParams {
        store: session_in(dir.path()),
        system_prompt:
            "You have the tools read, write, edit, and bash. Use them as                  instructed; edit takes the 3-char anchors from read output."
                .into(),
        model,
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig {
            max_output_tokens: Some(200),
            reasoning: Some(crate::provider::ReasoningEffort::Low),
        },
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send(
        "Do exactly this: 1) read notes.txt, 2) edit the line containing              'line2' so it becomes 'LINE2', 3) bash: cat notes.txt, 4) write              the file out.txt with the single line 'done'. Then reply              'finished'.",
        Lane::FollowUp,
    );
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let called: Vec<&str> = entries
        .iter()
        .filter(|e| e.kind == KIND_TOOL)
        .map(|e| e.payload["name"].as_str().unwrap())
        .collect();
    for want in ["read", "edit", "bash", "write"] {
        assert!(called.contains(&want), "missing {want}: {called:?}");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "line1\nLINE2\nline3\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "done"
    );
}

#[tokio::test]
async fn append_propagates_storage_errors_not_a_new_root() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("ok", &[])]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    // Lose the file under the session: the next append must fail with a
    // storage error, not silently start a new root branch.
    std::fs::remove_file(agent.inner.lock().unwrap().store.path()).unwrap();
    agent.send("again", Lane::FollowUp);
    assert!(agent.process().await.is_err());
}

#[tokio::test]
async fn runaway_turn_stops_with_a_visible_note() {
    let dir = tempfile::tempdir().unwrap();
    // The same tool call, forever: the round cap is the only exit.
    let body = sse(
        "",
        &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
    );
    let provider = Arc::new(ScriptedProvider::new(vec![body]));
    let agent = make_agent(dir.path(), provider);
    agent.send("loop", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let note = entries
        .iter()
        .find(|e| e.kind == KIND_SYSTEM)
        .expect("a system note records why the turn stopped");
    assert!(
        note.payload["note"].as_str().unwrap().contains("32"),
        "{note:?}"
    );
    assert_eq!(
        entries.iter().filter(|e| e.kind == KIND_TOOL).count(),
        MAX_ROUNDS
    );
}

#[tokio::test]
async fn kill_policy_completes_the_inflight_tool_batch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.txt"), "x\n").unwrap();
    // The killed stream had a completed function_call before the cut:
    // with the Complete policy the tool runs and the turn continues to
    // a final call where the forced message lands alongside the result.
    let mut body = String::new();
    body.push_str(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"c1\",\"type\":\"function_call\",\"name\":\"read\",\"call_id\":\"c1\",\"arguments\":\"{\\\"path\\\":\\\"x.txt\\\"}\"}}\n\n",
    );
    body.push_str("data: {\"type\":\"response.output_text.delta\",\"delta\":\"cut\"}\n\n");
    let body = body;
    let provider = canned_cut(&body, 1);
    let agent = AgentSession::new(SessionParams {
        store: session_in(dir.path()),
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.send("FORCE", Lane::Force);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
    // killed assistant + tool result + (the next scripted call = the
    // canned cut again, since the provider is single-shot) + the forced
    // user entry is delivered at the head of that final call.
    assert!(kinds.contains(&KIND_TOOL), "{kinds:?}");
    let interrupted = entries
        .iter()
        .find(|e| e.kind == KIND_ASSISTANT && e.payload["interrupted"].as_bool() == Some(true))
        .unwrap();
    assert_eq!(interrupted.payload["calls"].as_array().unwrap().len(), 1);
}
