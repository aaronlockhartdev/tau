use super::*;

use crate::harness::testkit::*;

fn write_skill_fixture(project: &Path, rel: &str, raw: &str) {
    let path = project.join(rel).join("SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, raw).unwrap();
}

/// `skill_list` serves the workspace's registry (discovered at command
/// time — no session needs to be open): project beats system, and a
/// `disable-model-invocation` skill stays listed (the dropdown is its
/// only door).
#[tokio::test]
async fn skill_list_serves_the_workspace_registry_with_project_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let system = tempfile::tempdir().unwrap();
    write_skill_fixture(
        system.path(),
        "skills/shared",
        "---\nname: shared\ndescription: system shared\n---\nBody.\n",
    );
    write_skill_fixture(
        tmp.path(),
        ".tau/skills/shared",
        "---\nname: shared\ndescription: project shared\n---\nBody.\n",
    );
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/other",
        "---\nname: other\ndescription: from .agents\ndisable-model-invocation: true\n---\nBody.\n",
    );
    let core = CoreBuilder::custom(providers())
        .with_system_dir(system.path().to_path_buf())
        .build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    match core
        .dispatch(Command::SkillList {
            workspace: workspace.id,
        })
        .unwrap()
    {
        CommandOutput::Skills { skills } => {
            assert_eq!(skills.len(), 2);
            let shared = skills.iter().find(|s| s.name == "shared").unwrap();
            assert_eq!(shared.description, "project shared");
            assert!(
                shared.location.starts_with(tmp.path().to_str().unwrap()),
                "the project file's location: {}",
                shared.location
            );
            let other = skills.iter().find(|s| s.name == "other").unwrap();
            assert!(!other.model_invocation);
        }
        other => panic!("expected skills: {other:?}"),
    }
}

/// The home-level `.agents/skills/` is part of the system scope (the
/// cross-client convention exists at both levels); a project
/// `.agents/skills/` skill of the same name shadows it.
#[tokio::test]

async fn skill_list_lists_the_home_agents_skill_and_the_project_one_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill_fixture(
        home.path(),
        ".agents/skills/shared",
        "---\nname: shared\ndescription: from the home .agents\n---\nBody.\n",
    );
    write_skill_fixture(
        home.path(),
        ".agents/skills/user-only",
        "---\nname: user-only\ndescription: only in the home .agents\n---\nBody.\n",
    );
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/shared",
        "---\nname: shared\ndescription: from the project .agents\n---\nBody.\n",
    );
    let core = CoreBuilder::custom(providers())
        .with_home(home.path().to_path_buf())
        .build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    match core
        .dispatch(Command::SkillList {
            workspace: workspace.id,
        })
        .unwrap()
    {
        CommandOutput::Skills { skills } => {
            assert_eq!(skills.len(), 2);
            let shared = skills.iter().find(|s| s.name == "shared").unwrap();
            assert_eq!(
                shared.description, "from the project .agents",
                "the project .agents skill shadows the home one"
            );
            let user_only = skills.iter().find(|s| s.name == "user-only").unwrap();
            assert!(
                user_only
                    .location
                    .starts_with(home.path().to_str().unwrap()),
                "the home file's location: {}",
                user_only.location
            );
        }
        other => panic!("expected skills: {other:?}"),
    }
}

/// `build_live` appends the skill catalog as the last layer of the
/// assembled prompt — after the context files, so a user AGENTS.md is
/// never drowned.
#[tokio::test]

async fn build_live_puts_the_catalog_after_the_context_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("AGENTS.md"), "project context").unwrap();
    write_skill_fixture(
        tmp.path(),
        ".tau/skills/alpha",
        "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id,
            title: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let live = core.live(&session.id).unwrap();
    let prompt = live.agent.system_prompt().to_owned();
    let ctx = prompt
        .find("project context")
        .expect("the project's AGENTS.md layer is present");
    let cat = prompt
        .find("<available_skills>")
        .expect("the catalog is present");
    assert!(
        ctx < cat,
        "the catalog lands after the context layer:\n{prompt}"
    );
    assert!(
        prompt.ends_with("</available_skills>"),
        "the catalog is the last layer:\n{prompt}"
    );
}
#[tokio::test]
async fn a_misspelled_skill_name_rejects_the_send() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/alpha",
        "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    let err = core
        .dispatch(Command::MessageSend {
            session: session_id.clone(),
            text: "/skill:alphax do it".into(),
            lane: MessageLane::Steering,
        })
        .unwrap_err();
    assert!(
        matches!(err, ProtocolError::Other { .. }),
        "expected a rejection, got: {err:?}"
    );
    let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &session_id);
    store.open().unwrap();
    assert!(
        store.entries_range(0, 100).unwrap().is_empty(),
        "a rejected send records nothing"
    );
}

/// A skill discovered into the registry, then removed from disk before
/// the send: the send rejects with an error naming the cause, and no
/// partial entry is recorded.
#[tokio::test]

async fn a_skill_removed_from_disk_before_send_rejects_the_send() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/alpha",
        "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    // Discovered while the file exists: it is now in the registry.
    match core
        .dispatch(Command::SkillList {
            workspace: workspace.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Skills { skills } => {
            assert!(skills.iter().any(|s| s.name == "alpha"));
        }
        other => panic!("expected skills: {other:?}"),
    }
    // Then the SKILL.md disappears before the send.
    std::fs::remove_file(
        tmp.path()
            .join(".agents")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
    )
    .unwrap();
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    let err = core
        .dispatch(Command::MessageSend {
            session: session_id.clone(),
            text: "/skill:alpha do it".into(),
            lane: MessageLane::Steering,
        })
        .unwrap_err();
    match &err {
        ProtocolError::Other { message } => {
            assert!(
                message.contains("reading"),
                "the error names the read failure: {message}"
            );
            assert!(
                message.contains("alpha"),
                "the error names the skill file: {message}"
            );
        }
        other => panic!("expected a read-failure rejection, got: {other:?}"),
    }
    let mut store = SessionStore::for_workspace(Path::new(&workspace.cwd), &session_id);
    store.open().unwrap();
    assert!(
        store.entries_range(0, 100).unwrap().is_empty(),
        "a rejected send records nothing"
    );
}

/// A valid `/skill:<name>` records the expansion template exactly
/// (the no-args variant omits the final line), with the payload
/// marker; a non-skill leading `/…` message is recorded verbatim.
#[tokio::test]

async fn a_skill_invocation_records_the_expanded_entry() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill_fixture(
        tmp.path(),
        ".agents/skills/alpha",
        "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let dir = tmp.path().join(".agents").join("skills").join("alpha");
    let location = dir.join("SKILL.md").to_string_lossy().into_owned();

    // With args: the template plus the `User request:` line.
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let s1 = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: s1.clone(),
        text: "/skill:alpha do it".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let entries = wait_for_user_entry(&workspace, &s1).await;
    let user = entries
        .iter()
        .find(|e| e.kind == crate::agent::KIND_USER)
        .unwrap();
    assert_eq!(
        user.payload["text"],
        format!(
            "Skill `alpha` — follow the instructions below. The skill directory is {}; resolve relative paths in the instructions against it.\n\nDo alpha.\n\nUser request: do it",
            dir.display()
        )
    );
    assert_eq!(user.payload["skill"]["name"], "alpha");
    assert_eq!(user.payload["skill"]["location"], location);

    // Bare invocation: the final line is omitted.
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let s2 = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: s2.clone(),
        text: "/skill:alpha".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let entries = wait_for_user_entry(&workspace, &s2).await;
    let user = entries
        .iter()
        .find(|e| e.kind == crate::agent::KIND_USER)
        .unwrap();
    assert_eq!(
        user.payload["text"],
        format!(
            "Skill `alpha` — follow the instructions below. The skill directory is {}; resolve relative paths in the instructions against it.\n\nDo alpha.",
            dir.display()
        )
    );

    // A non-skill leading slash is prose: recorded verbatim.
    let live = manual_session(
        &core,
        &workspace,
        provider::canned(&canned_body()),
        TurnConfig::default(),
    );
    let s3 = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: s3.clone(),
        text: "/not-a-skill".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let entries = wait_for_user_entry(&workspace, &s3).await;
    let user = entries
        .iter()
        .find(|e| e.kind == crate::agent::KIND_USER)
        .unwrap();
    assert_eq!(user.payload["text"], "/not-a-skill");
    assert!(user.payload.get("skill").is_none());
}

/// A /skill: send queued mid-turn shows in the GUI queue as the text the
/// turn will record — the expanded template, not the raw line. The
/// in-flight turn absorbs the steering message and records the expansion;
/// the post-turn reconciliation matches the queued item by exact text and
/// removes it, so a raw entry would have ghosted.
#[tokio::test]
async fn a_skill_send_queued_mid_turn_shows_its_expanded_text() {
    let cwd = tempfile::tempdir().unwrap();
    write_skill_fixture(
        cwd.path(),
        ".agents/skills/alpha",
        "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
    );
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, cwd.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned_slow(&canned_body(), 600),
        TurnConfig::default(),
    );
    let session_id = live.meta.lock().unwrap().id.clone();
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "work".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // A skill send while the turn is in flight: it is the queued one.
    core.dispatch(Command::MessageSend {
        session: session_id.clone(),
        text: "/skill:alpha do it".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    // The queued item shows the recorded text, not the raw line.
    let items = live.queue.lock().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert!(
        items[0].text.starts_with("Skill `alpha`"),
        "the queue shows the expanded text: {}",
        items[0].text
    );
    assert!(
        !items[0].text.starts_with("/skill:"),
        "the raw line must not queue"
    );
    // When the turn settles, the reconciliation removes the item by exact
    // text — nothing ghosts.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if live.queue.lock().unwrap().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queued skill send ghosted: {:?}",
            live.queue.lock().unwrap()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    drop(core);
}
