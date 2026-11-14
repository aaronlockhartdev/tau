//! Behaviour lane steps for the project directory: the `.tau` directory tau claims,
//! its ignore file (R-workspace-06, 07), the report of that write, and the store
//! path inside it (R-session-18).
//!
//! The project is a git working tree, so "ignored by git" is read from `git status`
//! rather than from the file's text. The tree lives under the repo's `target/tmp`,
//! which `cargo clean` removes; `Then the project directory is removed` cleans it
//! after a scenario that passes.

use crate::steps::session::{EVENTS, SESSION, STATE, TURN, commit_tool_call, open_at};
use crate::steps::{assert_clean, report};
use crate::world::World;
use cucumber::{given, then, when};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What the scenario seeds as a user's edit, to be left untouched.
const EDITED_IGNORE: &str = "*\n!keep\n";

fn project_root(label: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string());
    World::repo_root()
        .join("target")
        .join("tmp")
        .join(format!("project-{label}-{}-{stamp}", std::process::id()))
}

/// Run git in `root`. `stdin` is null so git never waits on the lane's terminal.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git {}: {e}", args[0]))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// What git would take from the tree: one line per path it does not ignore.
/// `git check-ignore -v -- path`, the line naming what ignores it: its first field is
/// `<source>:<line>:<pattern>`. `None` is git's answer that nothing ignores the path.
/// The config is pinned on the command line, so a global `core.excludesFile` or
/// `status.showUntrackedFiles` cannot answer for reasons unrelated to tau's file.
fn ignores(root: &Path, path: &str) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "status.showUntrackedFiles=all",
            "-c",
            "core.excludesFile=/dev/null",
            "check-ignore",
            "-v",
            "--",
            path,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git check-ignore: {e}"))?;
    if output.status.success() {
        Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(format!(
            "git check-ignore: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn read_ignore(root: &Path) -> Result<String, String> {
    std::fs::read_to_string(root.join(".tau").join(".gitignore"))
        .map_err(|e| format!("read the ignore file: {e}"))
}

fn make_root(world: &mut World, label: &str) -> Option<PathBuf> {
    world.findings.clear();
    world.project = None;
    let root = project_root(label);
    if let Err(e) = std::fs::create_dir_all(&root) {
        report(world, format!("create {}: {e}", root.display()));
        return None;
    }
    world.project_root = root.clone();
    if let Err(e) = git(&root, &["init", "-q"]) {
        report(world, e);
        return None;
    }
    Some(root)
}

fn open_project(world: &mut World, root: &Path) {
    match tau_core::project::open(root) {
        Ok(project) => world.project = Some(project),
        Err(e) => report(world, format!("open {}: {e}", root.display())),
    }
}

#[given("a git project directory")]
async fn a_git_project_directory(world: &mut World) {
    let Some(root) = make_root(world, "open") else {
        return;
    };
    open_project(world, &root);
}

#[given("a git project directory with an edited ignore file")]
async fn an_edited_project(world: &mut World) {
    let Some(root) = make_root(world, "edited") else {
        return;
    };
    let tau_dir = root.join(".tau");
    if let Err(e) = std::fs::create_dir_all(&tau_dir) {
        report(world, format!("create {}: {e}", tau_dir.display()));
        return;
    }
    if let Err(e) = std::fs::write(tau_dir.join(".gitignore"), EDITED_IGNORE) {
        report(world, format!("seed the ignore file: {e}"));
        return;
    }
    open_project(world, &root);
}

#[when("the project is opened again")]
async fn opened_again(world: &mut World) {
    let root = world.project_root.clone();
    open_project(world, &root);
}

#[then("the tau directory is ignored by git")]
async fn ignored_by_git(world: &mut World) {
    // The file's job is to keep what tau writes inside `.tau` out of git's view, so
    // the scenario puts a file there and asks git what it would take.
    let root = world.project_root.clone();
    if let Err(e) = std::fs::write(root.join(".tau").join("state.db"), "bytes") {
        report(world, format!("write inside .tau: {e}"));
    }
    match ignores(&root, ".tau/state.db") {
        // Line 1, pattern `*`: the clause asks for a file containing only `*`, so
        // the source is checked down to the pattern and the file's own text.
        Ok(Some(line)) if line.starts_with(".tau/.gitignore:1:*") => match read_ignore(&root) {
            Ok(text) if text == "*\n" => {}
            Ok(text) => report(world, format!("the ignore file holds {text:?}")),
            Err(e) => report(world, e),
        },
        Ok(Some(line)) => report(
            world,
            format!(
                "something other than tau's file ignores .tau/state.db: {}",
                line.trim()
            ),
        ),
        // git ignoring nothing is the leak the clause's `Why:` names.
        Ok(None) => report(world, "git does not ignore .tau/state.db".to_string()),
        Err(e) => report(world, e),
    }
}

#[then("the ignore file is unchanged")]
async fn ignore_unchanged(world: &mut World) {
    match read_ignore(&world.project_root.clone()) {
        Ok(text) if text == EDITED_IGNORE => {}
        Ok(text) => report(world, format!("the ignore file holds {text:?}")),
        Err(e) => report(world, e),
    }
}

#[then("the write is disclosed")]
async fn names_the_ignore_file(world: &mut World) {
    match world.project.as_ref().and_then(|p| p.disclosure()) {
        Some(line) if line.contains(".tau/.gitignore") => {}
        Some(line) => report(world, format!("the disclosure reads {line:?}")),
        None => report(
            world,
            "the write of the ignore file is undisclosed".to_string(),
        ),
    }
}

#[then("the reopen discloses nothing")]
async fn names_nothing(world: &mut World) {
    match world.project.as_ref().and_then(|p| p.disclosure()) {
        None => {}
        Some(line) => report(world, format!("the reopen disclosed {line:?}")),
    }
}

#[when("the store opens at the project's state file")]
async fn store_at_project_state_file(world: &mut World) {
    let Some(project) = world.project.clone() else {
        report(world, "no project is open".to_string());
        return;
    };
    open_at(world, project.state_db.clone());
    commit_tool_call(world);
}

#[then("the store is the project's state file")]
async fn store_is_the_project_state_file(world: &mut World) {
    let Some(project) = world.project.clone() else {
        report(world, "no project is open".to_string());
        return;
    };
    // The clause names the path, so the scenario pins it and not merely its directory.
    let want = world.project_root.join(".tau").join("state.db");
    if project.state_db != want {
        report(
            world,
            format!(
                "the store path is {}, not {}",
                project.state_db.display(),
                want.display()
            ),
        );
    }
    if !project.state_db.is_file() {
        report(world, format!("no store at {}", project.state_db.display()));
    }
    // Read the store into owned values first, so the borrow ends before a report.
    let read = world.store.as_ref().map(|store| {
        (
            store.events(SESSION, TURN).map(|rows| rows.len()),
            store.turn_state(SESSION, TURN),
        )
    });
    let Some((events, state)) = read else {
        report(world, "no store is open".to_string());
        assert_clean(world, "the store is the project's state file");
        return;
    };
    match events {
        Ok(rows) if rows == EVENTS.len() => {}
        Ok(rows) => report(
            world,
            format!("{rows} events are stored, expected {}", EVENTS.len()),
        ),
        Err(e) => report(world, format!("read the log: {e}")),
    }
    match state {
        Ok(Some(state)) if state == STATE => {}
        Ok(Some(state)) => report(world, format!("the stored state is {state}")),
        Ok(None) => report(world, "no turn state is stored".to_string()),
        Err(e) => report(world, format!("read the turn state: {e}")),
    }
    assert_clean(world, "the store is the project's state file");
}

#[then("the project directory is removed")]
async fn remove_project_directory(world: &mut World) {
    // This is the scenario's terminal step, so it is where the findings the earlier
    // steps collected are asserted. A red scenario leaves its tree behind, so the
    // failing files can be inspected.
    assert_clean(world, "the project directory is removed");
    world.project = None;
    // The store must close before its directory goes, or WAL sidecars stay open.
    world.store = None;
    let root = world.project_root.clone();
    // Refuse to remove anything outside this repo's `target/tmp`: both paths compared
    // Refuse to remove anything outside this repo's `target/tmp`: both paths compared
    // absolute, and never the shared directory itself.
    // shortcut: a lexical prefix check, enough while `project_root` only ever comes
    // from `project_root()`; canonicalize if a step ever takes a path from a feature.
    let want = World::repo_root().join("target").join("tmp");
    if root == PathBuf::new() || !root.starts_with(&want) || root == want {
        report(world, format!("refusing to remove {}", root.display()));
    } else if let Err(e) = std::fs::remove_dir_all(&root) {
        report(world, format!("remove {}: {e}", root.display()));
    }
    world.project_root = PathBuf::new();
    // A refusal is a finding, and this is the last chance to assert it.
    assert_clean(world, "the project directory is removed");
}
