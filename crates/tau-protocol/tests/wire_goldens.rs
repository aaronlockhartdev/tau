//! Wire-shape goldens for the protocol surface (ADR-0006): the exact JSON
//! each principal type emits is the GUI's contract — `protocol.ts` mirrors
//! it and `protocol-parity.mjs` checks the command names, but only a
//! snapshot pins the full field-level shape. Values are fixed test
//! constants, so no redactions are needed; a changed `.snap` means the wire
//! shape moved and the TS mirror must move with it.
#![allow(clippy::unwrap_used, clippy::panic)]

use serde_json::json;
use tau_protocol::EntryKind;
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
        kind: EntryKind::Assistant,
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
            duration_secs: Some(1.5),
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
    use tau_protocol::{CommandOutput, ContextMode, EntryKind, SkillInfo, SubagentInfo};

    #[test]
    fn snapshot_output() {
        let output = CommandOutput::Snapshot {
            snapshot: Snapshot {
                workspace: ws(),
                session: meta(),
                entries: vec![EntryMeta {
                    id: "00000007".into(),
                    parent: None,
                    kind: EntryKind::Assistant,
                    timestamp: 1_700_000_000_123,
                    size: 42,
                    preview: "hello".into(),
                    first_kept: None,
                    status: EntryStatus::Ok,
                }]
                .into(),
                om: OmSnapshot {
                    observation_tokens: 18_000,
                    pending_tokens: 1_000,
                    reflector_threshold: 40_000,
                },
                live: LiveState {
                    queue: vec![],
                    turn: TurnState::Running,
                    gate_failed: false,
                    plan: tau_protocol::snapshot::Plan {
                        steps: vec![
                            tau_protocol::snapshot::PlanStep {
                                text: "investigate the failure".into(),
                                done: true,
                            },
                            tau_protocol::snapshot::PlanStep {
                                text: "fix the parser".into(),
                                done: false,
                            },
                        ],
                        criteria: "build passes".into(),
                        notes: "the parser is the culprit".into(),
                    },
                    subagents: vec![SubagentInfo {
                        handle: "s1-1".into(),
                        child: "s2".into(),
                        agent_type: "general".into(),
                        context_mode: ContextMode::Fresh,
                        state: "running".into(),
                        waiting_on: None,
                        last_message: None,
                        usage: None,
                        resume_contract: None,
                        created: 1,
                    }],
                    own_state: None,
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
        AssistantPayload, FunctionCall, ImageBlock, ResumeContract, ToolPayload,
    };
    use tau_protocol::{Diagnostic, StreamOutcome};

    #[test]
    fn assistant_payload() {
        let p = AssistantPayload {
            text: "done".into(),
            reasoning: String::new(),
            interrupted: false,
            stop_reason: Some(StreamOutcome::Completed),
            usage: None,
            duration_secs: None,
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
                note: None,
            }),
            ui_detail: None,
            status: None,
        };
        assert_json_snapshot!(p);
    }

    #[test]
    fn resume_contract() {
        let c = ResumeContract {
            next_action: "fix the parser".into(),
        };
        assert_json_snapshot!(c);
    }

    #[test]
    fn diagnostic_payloads() {
        // The code spellings are append-only (ADR-0034): this snapshot is the
        // shape a GUI and a future core agree on.
        assert_json_snapshot!(vec![
            Diagnostic::ModelChanged {
                from: Some("qwen3.8-27b".into()),
                to: "qwen3.8-flash-next-iq3_s".into(),
            },
            Diagnostic::ModelChanged {
                from: None,
                to: "qwen3.8-flash-next-iq3_s".into(),
            },
            Diagnostic::GateUnavailable {
                model: "qwen3.8-flash-next-iq3_s".into(),
                missing: vec!["context_window".into(), "max_tokens".into()],
            },
            Diagnostic::ContextOverflow {
                prompt: 120_000,
                budget: 110_592,
            },
            Diagnostic::GateAttempt {
                command: "cargo nextest run".into(),
                retries: 1,
                max: 2,
                output: "test failed".into(),
            },
            Diagnostic::GateFailed {
                command: "cargo nextest run".into(),
                retries: 2,
            },
            Diagnostic::OmStalled {
                pending: 24_000,
                activation: 24_000,
            },
            Diagnostic::ProviderRejected {
                status: 400,
                body: "context length exceeded".into(),
            },
            Diagnostic::TurnCutoff {
                model: "qwen3.8-flash-next-iq3_s".into(),
                ended_by: StreamOutcome::EofWithoutDone,
                blocks: 2,
            },
        ]);
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
