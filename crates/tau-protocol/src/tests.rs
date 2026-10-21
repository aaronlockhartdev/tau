use super::*;
use serde_json::json;

/// Every command variant round-trips through its golden JSON shape.
#[test]
#[allow(clippy::too_many_lines)] // exhaustive per-variant round trip; splitting is refactoring
fn command_variants_roundtrip() {
    let commands = vec![
        Command::WorkspaceOpen {
            cwd: "/tmp/w".into(),
        },
        Command::WorkspaceList,
        Command::WorkspaceClose {
            workspace: "w1".into(),
        },
        Command::SessionList {
            workspace: "w1".into(),
        },
        Command::SessionNew {
            workspace: "w1".into(),
            title: None,
            base_prompt: None,
        },
        Command::SessionRename {
            session: "s1".into(),
            title: "Brave Otter".into(),
        },
        Command::SessionSetModel {
            session: "s1".into(),
            model: "m2".into(),
        },
        Command::SessionOpen {
            session: "s1".into(),
        },
        Command::SessionClose {
            session: "s1".into(),
        },
        Command::SessionDelete {
            session: "s1".into(),
        },
        Command::SessionArchive {
            session: "s1".into(),
        },
        Command::SessionRestore {
            workspace: "w1".into(),
            session: "s1".into(),
        },
        Command::SessionFork {
            session: "s1".into(),
            at: "00000042".into(),
        },
        Command::SessionBranch {
            session: "s1".into(),
            at: "00000042".into(),
        },
        Command::SessionSnapshot {
            session: "s1".into(),
        },
        Command::SessionEntries {
            session: "s1".into(),
            since: None,
            range: Some(snapshot::EntryRange {
                start: 0,
                count: 100,
            }),
        },
        Command::BlobRead {
            workspace: "w1".into(),
            id: "00000063".into(),
            hash: "0".repeat(16),
        },
        Command::MessageSend {
            session: "s1".into(),
            text: "hi".into(),
            lane: MessageLane::Steering,
        },
        Command::MessageStop {
            session: "s1".into(),
        },
        Command::SubagentTypes,
        Command::SubagentList {
            session: "s1".into(),
        },
        Command::SubagentState {
            handle: "h1".into(),
        },
        Command::SubagentSpawn {
            session: "s1".into(),
            agent_type: "general".into(),
            brief: "do the thing".into(),
            context_mode: ContextMode::Fresh,
        },
        Command::SubagentMessage {
            handle: "h1".into(),
            text: None,
        },
        Command::SubagentStop {
            handle: "h1".into(),
        },
        Command::SkillList {
            workspace: "w1".into(),
        },
        Command::ProviderList,
        Command::ProviderAdd {
            name: "p".into(),
            base_url: "u".into(),
            key_env: "K".into(),
            models: vec!["m".into()],
        },
        Command::ProviderSet {
            name: "p".into(),
            base_url: "u".into(),
            key_env: "K".into(),
            models: vec!["m".into()],
        },
        Command::ProviderDelete { name: "p".into() },
        Command::FileRead {
            workspace: "w1".into(),
            path: "a/b.txt".into(),
            offset: None,
            limit: None,
        },
        Command::FileList {
            workspace: "w1".into(),
            path: "src".into(),
        },
        Command::FileUnlist {
            workspace: "w1".into(),
            path: "src".into(),
        },
        Command::GitStatus {
            workspace: "w1".into(),
        },
    ];
    for cmd in &commands {
        let json = serde_json::to_string(cmd).unwrap();
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, cmd, "round-trip mismatch for {json}");
    }
}

#[test]
fn command_tag_shape() {
    let json = serde_json::to_string(&Command::MessageSend {
        session: "s1".into(),
        text: "hi".into(),
        lane: MessageLane::Force,
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"type":"message_send","session":"s1","text":"hi","lane":"force"}"#
    );
}
/// Every `CommandOutput` variant serializes to JSON. Regression: the
/// list variants were newtypes, and an internally tagged enum cannot
/// serialize a sequence payload — `workspace_list` failed on the wire
/// and the app launched with zero workspaces.
#[test]
#[allow(clippy::too_many_lines)] // exhaustive per-variant round trip; splitting is refactoring
fn command_output_variants_roundtrip() {
    let ws = snapshot::Workspace {
        id: "w1".into(),
        name: "tau".into(),
        cwd: "/tmp/tau".into(),
    };
    let meta = snapshot::SessionMeta {
        id: "s1".into(),
        workspace: "w1".into(),
        title: None,
        parent: None,
        created: 1,
        leaf: None,
        model: None,
        usage: None,
        archived: true,
    };
    let entry = snapshot::ViewEntry {
        id: "e1".into(),
        parent: None,
        kind: "message".into(),
        timestamp: 1,
        payload: json!({ "text": "hi" }),
        blob: None,
        first_kept: None,
    };
    let snap = snapshot::Snapshot {
        workspace: ws.clone(),
        session: meta.clone(),
        entries: vec![],
        om: snapshot::OmSnapshot {
            observation_tokens: 18_000,
            pending_tokens: 1_000,
            reflector_threshold: 40_000,
        },
        live: snapshot::LiveState {
            queue: vec![],
            turn: snapshot::TurnState::Idle,
            gate_failed: false,
            plan: snapshot::Plan::default(),
            subagents: vec![],
            own_state: None,
        },
        cursor: "e1".into(),
    };
    let provider = ProviderInfo {
        name: "vllm".into(),
        base_url: "http://localhost:8000/v1".into(),
        models: vec!["m1".into()],
    };
    let agent = AgentType {
        name: "general".into(),
        description: "the built-in".into(),
        builtin: true,
    };
    let sub = SubagentInfo {
        handle: "h1".into(),
        child: "s2".into(),
        agent_type: "general".into(),
        context_mode: ContextMode::Fresh,
        state: "running".into(),
        waiting_on: None,
        last_message: None,
        usage: None,
        resume_contract: None,
        created: 1,
    };
    let file = FileText {
        text: "x".into(),
        truncated: false,
    };
    let outputs = vec![
        CommandOutput::None,
        CommandOutput::Workspace { workspace: ws },
        CommandOutput::Workspaces { workspaces: vec![] },
        CommandOutput::Session { session: meta },
        CommandOutput::Sessions { sessions: vec![] },
        CommandOutput::Snapshot { snapshot: snap },
        CommandOutput::Entries {
            entries: vec![entry],
        },
        CommandOutput::Blob {
            payload: json!({ "active_observations": "obs" }),
        },
        CommandOutput::Providers {
            providers: vec![provider],
        },
        CommandOutput::Agents {
            agents: vec![agent],
        },
        CommandOutput::Skills {
            skills: vec![SkillInfo {
                name: "s".into(),
                description: "d".into(),
                location: "/p/.agents/skills/s/SKILL.md".into(),
                model_invocation: false,
            }],
        },
        CommandOutput::Subagents {
            subagents: vec![sub.clone()],
        },
        CommandOutput::Subagent { subagent: sub },
        CommandOutput::File { file },
        CommandOutput::Files {
            files: vec![FileEntry {
                name: "d".into(),
                path: "d".into(),
                dir: true,
                size: 0,
                ignored: false,
                git: None,
            }],
        },
    ];
    for out in &outputs {
        let json = serde_json::to_string(out).unwrap();
        let back: CommandOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, out, "round-trip mismatch for {json}");
    }
}

/// The tagged-union contract on the negative side: an unknown `type` tag
/// is a data error (the GUI's parity script and the core's dispatch both
/// rely on Err, never a panic, for an off-surface command).
#[test]
fn command_unknown_tag_is_data_error() {
    let err = serde_json::from_str::<Command>(r#"{"type":"bogus"}"#).unwrap_err();
    assert_eq!(err.classify(), serde_json::error::Category::Data);
}

#[test]
fn command_malformed_json_is_syntax_error() {
    let err = serde_json::from_str::<Command>(r#"{"type":"workspace_list"} x"#).unwrap_err();
    assert_eq!(err.classify(), serde_json::error::Category::Syntax);
}

#[test]
fn command_missing_required_field_is_data_error() {
    // message_send without its text/lane: a truncated or hand-edited
    // command must fail decode, not dispatch as something else.
    let err =
        serde_json::from_str::<Command>(r#"{"type":"message_send","session":"s1"}"#).unwrap_err();
    assert_eq!(err.classify(), serde_json::error::Category::Data);
}

#[test]
fn command_output_unknown_kind_is_data_error() {
    let err = serde_json::from_str::<CommandOutput>(r#"{"kind":"bogus"}"#).unwrap_err();
    assert_eq!(err.classify(), serde_json::error::Category::Data);
}

/// The error's wire shape is the transport contract: every variant tags
/// on `kind`, and an unknown kind refuses to decode.
#[test]
fn protocol_error_wire_shape() {
    let cases = vec![
        (
            ProtocolError::Unsupported {
                message: "m".into(),
            },
            r#"{"kind":"unsupported","message":"m"}"#,
        ),
        (
            ProtocolError::NotFound { what: "w".into() },
            r#"{"kind":"not_found","what":"w"}"#,
        ),
        (
            ProtocolError::Other {
                message: "m".into(),
            },
            r#"{"kind":"other","message":"m"}"#,
        ),
    ];
    for (err, wire) in cases {
        assert_eq!(&serde_json::to_string(&err).unwrap(), wire);
        let back: ProtocolError = serde_json::from_str(wire).unwrap();
        assert_eq!(back, err);
    }
    let err = serde_json::from_str::<ProtocolError>(r#"{"kind":"bogus"}"#).unwrap_err();
    assert_eq!(err.classify(), serde_json::error::Category::Data);
}

#[test]
fn lane_and_context_mode_reject_unknown_tags() {
    for bad in ["urgent", "force ", "FO"] {
        let json = format!("\"{bad}\"");
        let err = serde_json::from_str::<MessageLane>(&json).unwrap_err();
        assert_eq!(err.classify(), serde_json::error::Category::Data);
    }
    for bad in ["fresh", "compacted", "fork"] {
        let json = format!("\"{bad}\"");
        let back: ContextMode = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), format!("\"{bad}\""));
    }
    let err = serde_json::from_str::<ContextMode>(r#""clone""#).unwrap_err();
    assert_eq!(err.classify(), serde_json::error::Category::Data);
}

/// `Usage` is #[serde(default)]: a partial usage object from a deviating
/// server must decode with zeros, not fail the whole event.
#[test]
fn subagent_info_created_defaults_when_missing() {
    // A pre-`created` wire payload (an older core's subagent event) must
    // still deserialize: the field defaults to 0, which the spawn-order
    // sort treats as "no timestamp" (all-defaults tie on the child id).
    let json = r#"{"handle":"h1","child":"s2","agent_type":"general","context_mode":"fresh","state":"running","waiting_on":null,"last_message":null,"usage":null,"resume_contract":null}"#;
    let i: SubagentInfo = serde_json::from_str(json).unwrap();
    assert_eq!(i.created, 0);
    assert_eq!(i.child, "s2");
}

#[test]
fn usage_defaults_when_fields_missing() {
    let u: Usage = serde_json::from_str(r#"{"input_tokens":5}"#).unwrap();
    assert_eq!(
        u,
        Usage {
            input_tokens: 5,
            ..Usage::default()
        }
    );
    let u: Usage = serde_json::from_str("{}").unwrap();
    assert_eq!(u, Usage::default());
}
