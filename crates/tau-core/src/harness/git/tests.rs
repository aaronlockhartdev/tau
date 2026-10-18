use super::*;

fn ignore_search(patterns: &[&str]) -> gix::ignore::Search {
    gix::ignore::Search::from_overrides(patterns.to_vec(), gix::ignore::search::Ignore::default())
}

fn entry(name: &str, path: &str, dir: bool) -> FileEntry {
    FileEntry {
        name: name.into(),
        path: path.into(),
        dir,
        size: 0,
        ignored: false,
        git: None,
    }
}

#[test]
fn annotate_marks_ignored_and_git_state() {
    let snap = GitSnapshot {
        status: GitStatus {
            branch: "main".into(),
            modified: vec!["src/a.rs".into()],
            untracked: vec!["new.md".into()],
            deleted: vec![],
        },
        numstat: {
            let mut m = HashMap::new();
            m.insert("src/a.rs".to_owned(), (Some(3), Some(1)));
            m
        },
        ignore: ignore_search(&["target/"]),
        tracked_dirs: HashSet::new(),
        shown_dirs: HashSet::new(),
        offset: String::new(),
    };
    let mut files = vec![
        entry("a.rs", "src/a.rs", false),
        entry("new.md", "new.md", false),
        entry("x", "target/x", false),
    ];
    annotate(&mut files, Some(&snap));
    assert_eq!(
        files[0].git,
        Some(FileGit {
            state: FileGitState::Modified,
            added: Some(3),
            deleted: Some(1)
        })
    );
    assert_eq!(
        files[1].git,
        Some(FileGit {
            state: FileGitState::Untracked,
            added: None,
            deleted: None
        })
    );
    assert!(files[2].ignored && files[2].git.is_none());
}

#[test]
fn a_dir_whose_only_content_is_ignored_is_greyed() {
    // `.pi` is not itself ignored, but it holds no tracked and no
    // shown-untracked file - its sole child `.pi/skills` is ignored - so it
    // is greyed like its child, while `crates` (tracked content) is not.
    let snap = GitSnapshot {
        status: GitStatus {
            branch: "main".into(),
            modified: vec![],
            untracked: vec![],
            deleted: vec![],
        },
        numstat: HashMap::new(),
        ignore: ignore_search(&[".pi/skills/"]),
        tracked_dirs: {
            let mut s = HashSet::new();
            s.insert("crates".to_owned());
            s
        },
        shown_dirs: HashSet::new(),
        offset: String::new(),
    };
    let mut files = vec![
        entry("pi", ".pi", true),
        entry("skills", ".pi/skills", true),
        entry("crates", "crates", true),
    ];
    annotate(&mut files, Some(&snap));
    assert!(
        files[0].ignored,
        ".pi is greyed: its only content is ignored"
    );
    assert!(files[1].ignored, ".pi/skills is greyed: directly ignored");
    assert!(
        !files[2].ignored,
        "crates is not greyed: it has tracked content"
    );
}

#[test]
fn a_path_under_an_ignored_dir_is_ignored() {
    let snap = GitSnapshot {
        status: GitStatus {
            branch: "main".into(),
            modified: vec![],
            untracked: vec![],
            deleted: vec![],
        },
        numstat: HashMap::new(),
        ignore: ignore_search(&[".tau/", "node_modules/"]),
        tracked_dirs: HashSet::new(),
        shown_dirs: HashSet::new(),
        offset: String::new(),
    };
    // `.tau/skills` is empty and never in the pattern list, but its
    // ancestor `.tau` is ignored, so the whole subtree is greyed.
    assert!(is_ignored(".tau", true, &snap));
    assert!(is_ignored(".tau/skills", true, &snap));
    assert!(is_ignored("node_modules/pkg.js", false, &snap));
    assert!(!is_ignored("crates", true, &snap));
    assert!(!is_ignored("crates/tau-core", true, &snap));
}

#[test]
fn annotate_is_a_noop_without_a_snapshot() {
    let mut files = vec![entry("a.rs", "a.rs", false)];
    annotate(&mut files, None);
    assert!(!files[0].ignored && files[0].git.is_none());
}

#[test]
fn line_diff_counts_added_and_deleted_lines() {
    // b→x is a replace (1 delete + 1 insert); the trailing y is an insert.
    let (added, deleted) = line_diff_counts("a\nb\nc\n", "a\nx\nc\ny\n");
    assert_eq!((added, deleted), (2, 1));
}
