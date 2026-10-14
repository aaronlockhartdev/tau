//! Throwaway P0 spike: confirm the gix 0.88 API shape against THIS repo before
//! committing to it. Delete after the migration is green. Not a test — it
//! reads the real working tree and prints, so run with `cargo run`.
#![allow(
    // throwaway API spike, not production code
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::match_wildcard_for_single_variants,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_debug_formatting,
    clippy::unwrap_used,
)]
use std::time::Instant;

fn main() {
    let cwd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cwd = cwd.canonicalize().unwrap();
    println!("repo: {}", cwd.display());

    let repo = gix::open(&cwd).unwrap();
    let work_dir = repo.workdir().unwrap().to_path_buf();
    println!("opened ok; work_dir = {:?}", work_dir);

    // (b) ignore checks — the crux of the fix (works for empty dirs).
    let mut buf = Vec::new();
    let parse = gix::ignore::search::Ignore::default();
    let search = gix::ignore::Search::from_git_dir(
        repo.git_dir(),
        Some(work_dir.join(".gitignore")),
        &mut buf,
        parse,
    )
    .unwrap();
    let is_ignored = |p: &str, is_dir: bool| {
        let mut path = p.to_string();
        let mut first = true;
        loop {
            let as_dir = if first { is_dir } else { true };
            let m = search.pattern_matching_relative_path(
                gix::bstr::BStr::new(path.as_bytes()),
                Some(as_dir),
                gix::glob::pattern::Case::default(),
            );
            if m.is_some_and(|m| !m.pattern.is_negative()) {
                return true;
            }
            first = false;
            match path.rfind('/') {
                Some(i) => path.truncate(i),
                None => break,
            }
        }
        false
    };
    println!("is_ignored(target, dir)        = {}", is_ignored("target", true));
    println!("is_ignored(.tau, dir)          = {}", is_ignored(".tau", true));
    println!("is_ignored(.tau/skills, dir)   = {}", is_ignored(".tau/skills", true));
    println!("is_ignored(.tau/sessions, dir) = {}", is_ignored(".tau/sessions", true));
    println!("is_ignored(target/debug, dir)  = {}", is_ignored("target/debug", true));
    println!("is_ignored(crates, dir)        = {}", is_ignored("crates", true));
    println!("is_ignored(app, dir)           = {}", is_ignored("app", true));

    // (a) + (e) status walk, timed.
    let t = Instant::now();
    let platform = repo.status(gix::progress::Discard).unwrap();
    let iter = platform.into_iter(std::iter::empty::<gix::bstr::BString>()).unwrap();
    let setup_ms = t.elapsed().as_millis();

    let t2 = Instant::now();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut untracked = Vec::new();
    for res in iter {
        let item = res.unwrap();
        let gix::status::Item::IndexWorktree(iw) = item else {
            continue; // TreeIndex (staged) is out of scope for the spike
        };
        match iw {
            gix::status::index_worktree::Item::Modification {
                rela_path,
                status,
                ..
            } => {
                let path = String::from_utf8_lossy(&rela_path).into_owned();
                if let gix::status::plumbing::index_as_worktree::EntryStatus::Change(c) = status {
                    match c {
                        gix::status::plumbing::index_as_worktree::Change::Modification { .. }
                        | gix::status::plumbing::index_as_worktree::Change::Type { .. } => modified.push(path),
                        gix::status::plumbing::index_as_worktree::Change::Removed => deleted.push(path),
                        _ => {}
                    }
                }
            }
            gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
                untracked.push(String::from_utf8_lossy(&entry.rela_path).into_owned());
            }
            _ => {}
        }
    }
    println!(
        "status: setup {} ms + drain {} ms",
        setup_ms,
        t2.elapsed().as_millis()
    );

    modified.sort();
    untracked.sort();
    deleted.sort();
    println!("modified({}): {:?}", modified.len(), &modified[..modified.len().min(5)]);
    println!("deleted({}): {:?}", deleted.len(), &deleted[..deleted.len().min(5)]);
    println!("untracked({}): {:?}", untracked.len(), &untracked[..untracked.len().min(5)]);

    // branch name
    println!("head_name = {:?}", repo.head_name().ok());

    // (c) HEAD blob read (root-level file, via find_entry).
    let commit = repo.head_commit().unwrap();
    let tree = commit.tree().unwrap();
    let entry = tree.find_entry("Cargo.toml").unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    let head_cargo = String::from_utf8_lossy(&blob.data);
    println!("HEAD Cargo.toml first line: {:?}", head_cargo.lines().next());

    // (d) line-diff numstat on the first modified file.
    if let Some(rel) = modified.first() {
        let new = std::fs::read_to_string(work_dir.join(rel)).unwrap_or_default();
        let old = head_blob_at(&repo, &tree, rel);
        match old {
            Some(old) => {
                let diff = similar::TextDiff::from_lines(&old, new.as_str());
                let (mut added, mut del) = (0u32, 0u32);
                for op in diff.iter_all_changes() {
                    match op.tag() {
                        similar::ChangeTag::Insert => added += op.value().lines().count() as u32,
                        similar::ChangeTag::Delete => del += op.value().lines().count() as u32,
                        similar::ChangeTag::Equal => {}
                    }
                }
                println!("numstat({rel}): +{added} -{del}");
            }
            None => println!("numstat({rel}): not in HEAD tree"),
        }
    } else {
        println!("no modified files to numstat");
    }
}

/// Read a file's bytes at HEAD by walking the tree path segment by segment.
fn head_blob_at(repo: &gix::Repository, tree: &gix::Tree, rel: &str) -> Option<String> {
    let mut parts = rel.split('/');
    let seg = parts.next()?;
    let e = tree.find_entry(seg)?;
    match e.kind() {
        gix::object::tree::EntryKind::Tree => {
            let rest = parts.collect::<Vec<_>>().join("/");
            let next = repo.find_tree(e.id()).ok()?;
            head_blob_at(repo, &next, &rest)
        }
        gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
            let b = repo.find_blob(e.id()).ok()?;
            Some(String::from_utf8_lossy(&b.data).into_owned())
        }
        _ => None,
    }
}
