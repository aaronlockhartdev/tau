//! The container execution path (ticket #65): a docker-marked task's trials
//! run inside the task's own image — the TB environment model. The host
//! orchestrates the container lifecycle (create, copy in, exec, tear down);
//! a linux-built copy of this binary runs the trial in-container against the
//! image's /app, then the host records the trial's outcome and artifacts.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::EvalError;
use crate::live::Live;
use crate::runner::{Outcome, Status, Tokens, trial_dir};
use crate::task::Task;

/// The trial's result as carried across the container boundary (the
/// in-container `inner` mode prints it; the host parses it).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TrialResult {
    pub task: String,
    pub rep: u32,
    pub status: Status,
    pub score: bool,
    pub wall_ms: u64,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub turns: u32,
    pub tool_calls: Vec<String>,
    pub note: String,
}

impl TrialResult {
    #[must_use]
    pub fn from(o: &Outcome) -> Self {
        Self {
            task: o.task.clone(),
            rep: o.rep,
            status: o.status,
            score: o.score,
            wall_ms: o.wall_ms,
            tokens: o.tokens,
            cost_usd: o.cost_usd,
            turns: o.turns,
            tool_calls: o.tool_calls.clone(),
            note: o.note.clone(),
        }
    }
}

/// `docker <args>`; stdout on success, a diagnostic with stderr on failure.
async fn docker(args: &[&str]) -> Result<String, EvalError> {
    let out = Command::new("docker")
        .args(args)
        .output()
        .await
        .map_err(|e| EvalError::Live(format!("docker {args:?}: {e}")))?;
    if !out.status.success() {
        return Err(EvalError::Live(format!(
            "docker {args:?} exited {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The rust build image for `arch`: the local `rust:1.98-bookworm` when
/// its architecture matches, else the matching variant fetched by digest
/// and tagged `rust:1.98-bookworm-<arch>` — a same-named local image of the
/// other arch shadows a plain `docker pull`, so the digest is how the other
/// side gets fetched at all.
async fn build_image(arch: &str) -> Result<String, EvalError> {
    let base = "rust:1.98-bookworm";
    let want = if arch == "amd64" { "amd64" } else { "arm64" };
    let local_arch = docker(&["image", "inspect", base, "--format", "{{.Architecture}}"]).await?;
    if local_arch == want {
        return Ok(base.to_owned());
    }
    let tagged = format!("{base}-{want}");
    if docker(&["image", "inspect", &tagged, "--format", "{{.Id}}"])
        .await
        .is_ok()
    {
        return Ok(tagged);
    }
    eprintln!("tau-eval: first use — pulling the {want} rust image (a few hundred MB)");
    let manifest = docker(&["manifest", "inspect", base]).await?;
    let m: serde_json::Value =
        serde_json::from_str(&manifest).map_err(|e| EvalError::Live(format!("manifest: {e}")))?;
    let digest = m["manifests"]
        .as_array()
        .and_then(|list| {
            list.iter().find(|e| {
                e["platform"]["architecture"].as_str() == Some(want)
                    && e["platform"]["os"].as_str() == Some("linux")
            })
        })
        .and_then(|e| e["digest"].as_str())
        .ok_or_else(|| EvalError::Live(format!("no {want} manifest entry for {base}")))?;
    docker(&["pull", &format!("{base}@{digest}")]).await?;
    docker(&["tag", &format!("{base}@{digest}"), &tagged]).await?;
    Ok(tagged)
}

/// The linux build of this binary for the image's architecture, built on
/// first use: this host has no C cross-compiler (zstd, aws-lc), so the
/// build runs natively inside a matching rust container (foreign-arch
/// containers emulate — slower, but correct).
async fn ensure_binary(arch: &str) -> Result<PathBuf, EvalError> {
    let triple = match arch {
        // Static musl: the trial images span glibc vintages (and some ship
        // none at all), so the in-container binary carries no glibc floor.
        "amd64" => "x86_64-unknown-linux-musl",
        "arm64" => "aarch64-unknown-linux-musl",
        other => {
            return Err(EvalError::Live(format!(
                "container arch {other} has no cross-built tau-eval"
            )));
        }
    };
    let path = PathBuf::from(format!("target/{triple}/release/tau-eval"));
    if path.is_file() {
        return Ok(path);
    }
    let root = std::env::current_dir().map_err(|e| EvalError::Live(format!("current dir: {e}")))?;
    // The host share is read-only from the container's view, so the build
    // writes to container-local /build and the binary comes back via
    // `docker cp` (which writes through the daemon, not the mount).
    let mount = format!("{}:/src:ro", root.display());
    let builder = format!("taubuild-{}", {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    });
    eprintln!(
        "tau-eval: first use — building the in-container binary for {triple} (this takes a few minutes)"
    );
    let image = build_image(arch).await?;
    let build_cmd = format!(
        "apt-get update -qq && apt-get install -y -qq musl-tools && rustup target add {triple} && cargo build --target {triple} --release -p tau-eval"
    );
    // The build runs in the image variant of the target arch: the amd64 rust
    // image (under QEMU on arm64 hosts) ships the x86_64 toolchain, so no
    // in-container toolchain download is ever needed.
    let platform = if triple.starts_with("x86_64") {
        "linux/amd64"
    } else {
        "linux/arm64"
    };
    let args = vec![
        "run",
        "--platform",
        platform,
        "--name",
        &builder,
        "-v",
        &mount,
        "-w",
        "/src",
        "-e",
        // Matches the repo's rust-toolchain pin and the image tag above; explicit, so the in-container rustup does not re-sync from the network.
        "RUSTUP_TOOLCHAIN=1.98.1",
        "-e",
        "CARGO_TARGET_DIR=/build/target",
        &image,
        "sh",
        "-c",
        &build_cmd,
    ];
    let _ = docker(&args).await?;
    let host_path = path.canonicalize().unwrap_or_else(|_| path.clone());
    let container_path = format!("/build/target/{triple}/release/tau-eval");
    let cp_src = format!("{builder}:{container_path}");
    let cp_args = vec![
        "cp",
        &cp_src,
        host_path
            .to_str()
            .ok_or_else(|| EvalError::Live("target path is not valid UTF-8".into()))?,
    ];
    let _ = docker(&cp_args).await?;
    let _ = docker(&["rm", "-f", &builder]).await;
    if !path.is_file() {
        return Err(EvalError::Live(format!(
            "in-container build produced no {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Make sure the image is local: inspect it (learning its architecture),
/// pulling it first when absent.
async fn ensure_image(image: &str) -> Result<String, EvalError> {
    let inspect =
        || async { docker(&["image", "inspect", image, "-f", "{{.Architecture}}"]).await };
    let arch = inspect().await;
    if arch.is_err() {
        docker(&["pull", image])
            .await
            .map_err(|e| EvalError::Live(format!("image {image}: {e}")))?;
        return inspect().await;
    }
    arch
}

/// The TB-protocol staging: the task's `tests/` becomes `/tests` (the verdict
/// dir `/logs/verifier` is created with the other trial dirs).
async fn stage_tb_tests(name: &str, task: &Task) -> Result<(), EvalError> {
    docker(&[
        "cp",
        task.dir
            .join("tests")
            .to_str()
            .ok_or_else(|| EvalError::Live("task dir is not valid UTF-8".into()))?,
        &format!("{name}:/tests"),
    ])
    .await?;
    Ok(())
}
/// Copy the trial's inputs into the container and run `inner` in /app.
async fn run_in_container(
    name: &str,
    live: &Live,
    task: &Task,
    binary: &Path,
) -> Result<TrialResult, EvalError> {
    // Layout inside the container: the config under a private HOME, the task
    // at /tmp/task (created by `docker cp` — a pre-existing dir would nest
    // the copy), the trial artifacts at /tmp/trial, the workspace at /app
    // (created when the image has none).
    // Start first: `docker exec` needs a running container.
    docker(&["start", name]).await?;
    let mut mkdir: Vec<&str> = vec![
        "exec",
        name,
        "mkdir",
        "-p",
        "/app",
        "/tmp/tau-home/.config",
        "/tmp/trial",
    ];
    if task.tb_native {
        // /logs/verifier only: /tests must NOT pre-exist, or `docker cp`
        // nests the copy inside it.
        mkdir.push("/logs/verifier");
    }
    docker(&mkdir).await?;
    docker(&[
        "cp",
        binary
            .to_str()
            .ok_or_else(|| EvalError::Live("binary path is not valid UTF-8".into()))?,
        &format!("{name}:/tmp/tau-eval"),
    ])
    .await?;
    docker(&[
        "cp",
        live.scratch_config_dir()
            .to_str()
            .ok_or_else(|| EvalError::Live("config dir is not valid UTF-8".into()))?,
        &format!("{name}:/tmp/tau-home/.config/tau"),
    ])
    .await?;
    docker(&[
        "cp",
        task.dir
            .to_str()
            .ok_or_else(|| EvalError::Live("task dir is not valid UTF-8".into()))?,
        &format!("{name}:/tmp/task"),
    ])
    .await?;
    if task.tb_native {
        stage_tb_tests(name, task).await?;
    }
    // Minimal images often ship no CA store; the bundled Mozilla roots let
    // rustls reach the LLM endpoint.
    let cacert = Path::new("eval/cacert.pem");
    docker(&[
        "cp",
        cacert
            .to_str()
            .ok_or_else(|| EvalError::Live("cacert path is not valid UTF-8".into()))?,
        &format!("{name}:/tmp/ca.pem"),
    ])
    .await?;

    let mut exec_args: Vec<&str> = vec![
        "exec",
        "-w",
        "/app",
        "-e",
        "HOME=/tmp/tau-home",
        "-e",
        "SSL_CERT_FILE=/tmp/ca.pem",
        name,
        "/tmp/tau-eval",
        "inner",
        "--task",
        "/tmp/task",
        "--workspace",
        "/app",
    ];
    if task.tb_native {
        exec_args.push("--tb");
        exec_args.push("--id");
        exec_args.push(task.id());
    }
    let exec = Command::new("docker")
        .args(&exec_args)
        .output()
        .await
        .map_err(|e| EvalError::Live(format!("docker exec: {e}")))?;
    let stdout = String::from_utf8_lossy(&exec.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&exec.stderr).into_owned();
    // The result JSON is the last non-empty stdout line (the app logs to
    // stderr, so stdout is the channel). A failed trial is a *result* —
    // exit 1 with the JSON on stdout — so the JSON is parsed before the
    // exit status is treated as an error.
    match stdout.lines().rev().find(|l| !l.trim().is_empty()) {
        Some(json) => serde_json::from_str(json)
            .map_err(|e| EvalError::Live(format!("in-container result: {e}: {json}"))),
        None if !exec.status.success() => Err(EvalError::Live(format!(
            "in-container trial exited {}: {}",
            exec.status.code().unwrap_or(-1),
            stderr.trim()
        ))),
        None => Err(EvalError::Live(
            "in-container trial printed no result".into(),
        )),
    }
}

/// Run one container trial: create the container from the task's image,
/// copy in the binary, the scratch config, and the task dir, run `inner` in
/// /app, harvest the trial artifacts, tear down.
pub async fn run_trial(
    live: &Live,
    task: &Task,
    rep: u32,
    artifacts_dir: &Path,
) -> Result<Outcome, EvalError> {
    let image = task
        .docker()
        .ok_or_else(|| EvalError::Live(format!("task {} has no docker image", task.id())))?;
    let arch = ensure_image(image).await?;
    let binary = ensure_binary(&arch).await?;

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let name = format!("taueval-{nanos}-{}-{rep}", task.id());
    // Per-container artifact staging (trials run concurrently).
    let staging = format!("host-trial-tmp-{name}");
    docker(&[
        "create",
        "--entrypoint=",
        "--name",
        &name,
        "--network",
        "bridge",
        image,
        "sleep",
        "infinity",
    ])
    .await?;

    let result = run_in_container(&name, live, task, &binary).await;

    // Always tear the container down, success or failure.
    let _ = docker(&["rm", "-f", &name]).await;

    let trial = result?;
    let outcome = Outcome {
        task: trial.task,
        rep: trial.rep,
        status: trial.status,
        score: trial.score,
        // The in-container wall clock is the honest number; the host's adds
        // container orchestration, which is not the agent's time.
        wall_ms: trial.wall_ms,
        tokens: trial.tokens,
        cost_usd: trial.cost_usd,
        turns: trial.turns,
        tool_calls: trial.tool_calls,
        note: trial.note,
    };
    // Record the outcome in the host's artifacts dir (the session copy came
    // back in the staging dir when present).
    let host_trial = trial_dir(artifacts_dir, &outcome.task, outcome.rep);
    std::fs::create_dir_all(&host_trial).map_err(EvalError::io)?;
    let container_session = Path::new(&staging).join("session.jsonl");
    if container_session.is_file() {
        let _ = std::fs::copy(&container_session, host_trial.join("session.jsonl"));
        let _ = std::fs::remove_dir_all(&staging);
    }
    crate::report::write_results(&host_trial, &outcome).map_err(EvalError::io)?;
    Ok(outcome)
}
