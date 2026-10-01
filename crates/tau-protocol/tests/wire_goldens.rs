//! Wire-shape goldens for the protocol surface (ADR-0006): the exact JSON
//! each principal type emits is the GUI's contract — `protocol.ts` mirrors
//! it and `protocol-parity.mjs` checks the command names, but only a
//! snapshot pins the full field-level shape. Values are fixed test
//! constants, so no redactions are needed; a changed `.snap` means the wire
//! shape moved and the TS mirror must move with it.

use serde_json::json;
use tau_protocol::snapshot::{SessionMeta, ViewEntry, Workspace};

fn ws() -> Workspace {
    Workspace {
        id: "w1".into(),
        name: "tau".into(),
        cwd: "/tmp/tau".into(),
    }
}

fn meta() -> SessionMeta {
    SessionMeta {
        id: "s1".into(),
        workspace: "w1".into(),
        title: Some("Brave Otter".into()),
        parent: None,
        created: 1_700_000_000_000,
        leaf: Some("00000042".into()),
        model: Some("qwen3.8-27b".into()),
        usage: None,
        archived: false,
    }
}

fn view_entry() -> ViewEntry {
    ViewEntry {
        id: "00000007".into(),
        parent: Some("00000006".into()),
        kind: "assistant".into(),
        timestamp: 1_700_000_000_123,
        payload: json!({ "text": "hello" }),
        blob: None,
        first_kept: None,
    }
}

mod commands {
    use insta::assert_json_snapshot;
    use tau_protocol::Command;

    #[test]
    fn workspace_open() {
        assert_json_snapshot!(Command::WorkspaceOpen {
            cwd: "/tmp/tau".into()
        });
    }

    #[test]
    fn session_entries_with_range() {
        assert_json_snapshot!(Command::SessionEntries {
            session: "s1".into(),
            since: None,
            range: Some(tau_protocol::snapshot::EntryRange {
                start: 0,
                count: 100
            }),
        });
    }

    #[test]
    fn message_send() {
        assert_json_snapshot!(Command::MessageSend {
            session: "s1".into(),
            text: "hi".into(),
            lane: tau_protocol::MessageLane::Steering,
        });
    }

    #[test]
    fn subagent_spawn() {
        assert_json_snapshot!(Command::SubagentSpawn {
            session: "s1".into(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: tau_protocol::ContextMode::Compacted,
        });
    }

    #[test]
    fn task_evidence() {
        assert_json_snapshot!(Command::TaskEvidence {
            session: "s1".into(),
            task: "t-1".into(),
            criterion: "fast".into(),
            summary: "bench attached".into(),
            passed: Some(true),
        });
    }

    #[test]
    fn provider_add() {
        assert_json_snapshot!(Command::ProviderAdd {
            name: "vllm".into(),
            base_url: "http://localhost:8000/v1".into(),
            key_env: "VLLM_KEY".into(),
            models: vec!["qwen3.8-27b".into()],
        });
    }

    #[test]
    fn file_read() {
        assert_json_snapshot!(Command::FileRead {
            workspace: "w1".into(),
            path: "src/core.rs".into(),
            offset: Some(10),
            limit: Some(50),
        });
    }

    #[test]
    fn blob_read() {
        assert_json_snapshot!(Command::BlobRead {
            workspace: "w1".into(),
            id: "00000063".into(),
            hash: "0".repeat(16),
        });
    }
}

mod events {
    use crate::view_entry;
    use insta::assert_json_snapshot;
    use tau_protocol::payload::Task;
    use tau_protocol::snapshot::QueuedItem;
    use tau_protocol::{Event, Usage};

    #[test]
    fn stream_end_with_usage() {
        assert_json_snapshot!(Event::StreamEnd {
            workspace: "w1".into(),
            session: "s1".into(),
            call_id: "c1".into(),
            interrupted: false,
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 20,
                total_tokens: 120,
                cached_prompt_tokens: 40,
            }),
        });
    }

    #[test]
    fn entry_upsert() {
        assert_json_snapshot!(Event::EntryUpsert {
            workspace: "w1".into(),
            session: "s1".into(),
            entry: view_entry(),
        });
    }

    #[test]
    fn queue() {
        assert_json_snapshot!(Event::Queue {
            workspace: "w1".into(),
            session: "s1".into(),
            items: vec![QueuedItem {
                text: "next".into(),
                lane: tau_protocol::MessageLane::FollowUp,
                source: Some("s2".into()),
            }],
        });
    }

    #[test]
    fn subagent_spawned() {
        assert_json_snapshot!(Event::SubagentEvent {
            workspace: "w1".into(),
            session: "s1".into(),
            kind: tau_protocol::SubagentEventKind::Spawned {
                handle: "s1-1".into(),
                child: "s2".into(),
                agent_type: "general".into(),
                context_mode: tau_protocol::ContextMode::Fresh,
                title: "rusty-nail".into(),
            },
        });
    }

    #[test]
    fn task_changed() {
        assert_json_snapshot!(Event::TaskChanged {
            workspace: "w1".into(),
            session: "s1".into(),
            tasks: vec![Task {
                id: "t-1".into(),
                title: "do it".into(),
                status: "pending".into(),
                steps: vec![],
                criteria: vec![],
                evidence: vec![],
                blockers: vec![],
                decisions: vec![],
                notes: vec![],
                worker: None,
                created_in: None,
                updated: 0,
            }],
        });
    }

    #[test]
    fn file_tree_changed() {
        assert_json_snapshot!(Event::FileTreeChanged {
            workspace: "w1".into(),
            changed: vec!["src".into(), "src/core".into()],
        });
    }
}

mod outputs {
    use crate::{meta, ws};
    use insta::assert_json_snapshot;
    use tau_protocol::snapshot::{
        EntryMeta, EntryStatus, LiveState, OmSnapshot, Snapshot, TurnState,
    };
    use tau_protocol::{CommandOutput, ContextMode, SkillInfo, SubagentInfo};

    #[test]
    fn snapshot_output() {
        let output = CommandOutput::Snapshot {
            snapshot: Snapshot {
                workspace: ws(),
                session: meta(),
                entries: vec![EntryMeta {
                    id: "00000007".into(),
                    parent: None,
                    kind: "assistant".into(),
                    timestamp: 1_700_000_000_123,
                    size: 42,
                    preview: "hello".into(),
                    first_kept: None,
                    status: EntryStatus::Ok,
                }],
                om: OmSnapshot {
                    observation_tokens: 18_000,
                    pending_tokens: 1_000,
                    reflector_threshold: 40_000,
                },
                live: LiveState {
                    queue: vec![],
                    turn: TurnState::Running,
                    subagents: vec![SubagentInfo {
                        handle: "s1-1".into(),
                        child: "s2".into(),
                        agent_type: "general".into(),
                        context_mode: ContextMode::Fresh,
                        state: "running".into(),
                        waiting_on: None,
                        last_message: None,
                        usage: None,
                        task: None,
                        resume_contract: None,
                    }],
                    tasks: vec![],
                },
                cursor: "00000042".into(),
            },
        };
        assert_json_snapshot!(output);
    }

    #[test]
    fn blob_output() {
        assert_json_snapshot!(CommandOutput::Blob {
            payload: serde_json::json!({ "active_observations": "obs" }),
        });
    }

    #[test]
    fn skills_output() {
        assert_json_snapshot!(CommandOutput::Skills {
            skills: vec![SkillInfo {
                name: "s".into(),
                description: "d".into(),
                location: "/p/.agents/skills/s/SKILL.md".into(),
                model_invocation: false,
            }],
        });
    }
}

mod payloads {
    use insta::assert_json_snapshot;
    use tau_protocol::payload::{
        AssistantPayload, FunctionCall, ImageBlock, Step, StepStatus, Task, TaskEvent, ToolPayload,
    };

    fn task() -> Task {
        Task {
            id: "t-1".into(),
            title: "ship the parser".into(),
            status: "active".into(),
            steps: vec![Step {
                text: "write the parser".into(),
                expected_output: "tests pass".into(),
                status: StepStatus::Active,
            }],
            criteria: vec![],
            evidence: vec![],
            blockers: vec![],
            decisions: vec![],
            notes: vec!["started".into()],
            worker: None,
            created_in: Some("s1".into()),
            updated: 1_700_000_000_000,
        }
    }

    #[test]
    fn task_snapshot() {
        assert_json_snapshot!(task());
    }

    #[test]
    fn task_event_created() {
        assert_json_snapshot!(TaskEvent::Created {
            title: "ship the parser".into(),
            steps: vec![Step {
                text: "write the parser".into(),
                expected_output: "tests pass".into(),
                status: StepStatus::Pending,
            }],
            criteria: vec![],
        });
    }

    #[test]
    fn assistant_payload() {
        let p = AssistantPayload {
            text: "done".into(),
            reasoning: String::new(),
            interrupted: false,
            usage: None,
            calls: vec![FunctionCall {
                id: "f1".into(),
                call_id: "c1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"cargo test"}"#.into(),
            }],
        };
        assert_json_snapshot!(p);
    }

    #[test]
    fn tool_payload_image() {
        let p = ToolPayload {
            call_id: "c1".into(),
            name: "read".into(),
            args: serde_json::json!({ "path": "a.png" }),
            output: tau_protocol::payload::ToolOutput::Image(ImageBlock {
                kind: "image".into(),
                media_type: "image/png".into(),
                data_base64: "AAAA".into(),
            }),
        };
        assert_json_snapshot!(p);
    }

    #[test]
    fn resume_contract() {
        let c = tau_protocol::payload::resume_contract(&task());
        assert_json_snapshot!(c);
    }
}

mod errors {
    use insta::assert_json_snapshot;
    use tau_protocol::ProtocolError;

    #[test]
    fn all_variants() {
        assert_json_snapshot!(vec![
            ProtocolError::Unsupported {
                message: "subagents land with ticket #23".into()
            },
            ProtocolError::NotFound {
                what: "session s1".into()
            },
            ProtocolError::Other {
                message: "disk full".into()
            },
        ]);
    }
}
