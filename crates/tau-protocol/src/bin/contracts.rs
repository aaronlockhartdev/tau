//! Writes the generated wire schemas into `contracts/` (R-workspace-04).
//! Run from the repo root: `cargo run -p tau-protocol --bin contracts [-- --check]`.

use std::fs;
use std::path::Path;
use std::process::exit;

const DIR: &str = "contracts";

/// The generated set: file name to contents. Empty while `tau-protocol` declares
/// no wire type; the first type lands with the change that adds it.
fn generated() -> Vec<(String, String)> {
    Vec::new()
}

/// The files git tracks under `contracts/`. No git, or a repo that does not track
/// the directory, yields an empty list: every generated schema is then a finding.
fn git_tracked() -> Vec<String> {
    let Ok(out) = std::process::Command::new("git")
        .args(["ls-files", "-z", DIR])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Generated schemas that git does not track under `DIR`: the whole relative path
/// is compared, so `contracts/xfoo.json` does not stand in for `foo.json`.
fn uncommitted(want: &[String], tracked: &[String]) -> Vec<String> {
    want.iter()
        .filter(|name| !tracked.iter().any(|t| *t == format!("{DIR}/{name}")))
        .cloned()
        .collect()
}

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let want = generated();

    // The paths are repo-root relative, like the gate row that runs this binary.
    if !Path::new("crates").is_dir() {
        eprintln!("contracts: not a repo root: no crates/ here");
        exit(1);
    }
    let mut problems = 0;
    let mut stale: Vec<std::path::PathBuf> = Vec::new();
    for (name, contents) in &want {
        let path = Path::new(DIR).join(name);
        match fs::read_to_string(&path) {
            // Contents compared, so a schema that changed in shape cannot pass
            // with the same file name on disk.
            Ok(existing) if existing == *contents => {}
            Ok(_) => {
                println!("{DIR}/{name}: content differs from the generated schema");
                problems += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("{DIR}/{name}: missing");
                problems += 1;
            }
            // R-workspace-04: a schema that cannot be read cannot be certified.
            Err(e) => {
                println!("{DIR}/{name}: unreadable: {e}");
                problems += 1;
            }
        }
    }

    // R-workspace-04: generated with the types means committed with them.
    let tracked = git_tracked();
    let names: Vec<String> = want.iter().map(|(name, _)| name.clone()).collect();
    let missing_from_git = uncommitted(&names, &tracked);
    for name in &missing_from_git {
        println!("{DIR}/{name}: generated, not committed");
        problems += 1;
    }

    // An unreadable contracts/ is not an empty set of schemas. Write mode creates
    // the directory first so it self-heals a tree without one; check mode reports.
    if !check && let Err(e) = fs::create_dir_all(DIR) {
        eprintln!("contracts: {e}");
        exit(1);
    }
    let Ok(entries) = fs::read_dir(Path::new(DIR)) else {
        println!("{DIR}: unreadable directory");
        exit(1);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let json = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        let Some(name) = name else { continue };
        if json && !want.iter().any(|(wanted, _)| *wanted == name) {
            println!("{DIR}/{name}: on disk, not generated");
            problems += 1;
            stale.push(path);
        }
    }

    if check {
        println!("contracts: {} schema(s), {problems} problem(s)", want.len());
        exit(if problems == 0 { 0 } else { 1 });
    }

    for (name, contents) in &want {
        let path = Path::new(DIR).join(name);
        // Write-then-rename: a failed write leaves the previous schema readable.
        let tmp = path.with_extension("json.tmp");
        let written = fs::create_dir_all(DIR)
            .and_then(|()| fs::write(&tmp, contents))
            .and_then(|()| fs::rename(&tmp, &path));
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            eprintln!("contracts: {e}");
            exit(1);
        }
    }
    // Writing converges the directory: a schema no type generates is deleted. An
    // empty generated set proves nothing about the directory, so it deletes nothing.
    if !want.is_empty() {
        for path in &stale {
            if let Err(e) = fs::remove_file(path) {
                eprintln!("contracts: {e}");
                exit(1);
            }
        }
    }
    println!("contracts: wrote {} schema(s)", want.len());
    // Writing fixes contents and staleness, not a schema that is not committed.
    if !missing_from_git.is_empty() {
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{DIR, uncommitted};

    #[test]
    fn a_schema_is_committed_only_under_its_own_path() {
        let tracked = [format!("{DIR}/xfoo.json"), format!("{DIR}/bar.json")];
        let want = ["foo.json".to_string(), "bar.json".to_string()];
        assert_eq!(uncommitted(&want, &tracked), vec!["foo.json".to_string()]);
    }
}
