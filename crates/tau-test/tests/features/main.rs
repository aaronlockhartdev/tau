//! The behaviour lane (R-harness-04): cucumber runs the feature tree from the
//! crate root, so `cargo test -p tau-test --test features` reaches it.

use cucumber::World as _;
use std::path::Path;
use tau_test::world::World as Lane;

#[tokio::main]
async fn main() {
    // The behaviour lane is cucumber, not the libtest harness. nextest lists every
    // test binary in the workspace, and this one names no libtest test.
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    // Cucumber's exit status counts only failures, parse and hook errors: a lane
    // that runs nothing (empty tree, empty file, @wip) exits 0. Counted here, and a
    // skipped scenario is a failure. A hand-run `--tags` filter that selects nothing
    // still exits 0: that is cucumber's behaviour, and the gate command passes none.
    let (files, scenarios) = tree(Path::new("tests/features"));
    if files == 0 || scenarios == 0 {
        panic!(
            "no scenario under tests/features ({files} .feature file(s)): the behaviour lane would run nothing"
        );
    }
    Lane::cucumber()
        .fail_on_skipped()
        .run_and_exit("tests/features")
        .await;
}

/// `.feature` files and `Scenario` headings, over the whole tree: cucumber's
/// directory input walks subdirectories too (measured here — a feature in a
/// subdirectory ran, so the count must cover it).
/// shortcut: a `Scenario Outline` with an empty `Examples` table is counted here
/// and runs nothing; upgrade if the tree grows an outline.
fn tree(dir: &Path) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    entries.flatten().fold((0, 0), |acc, entry| {
        let path = entry.path();
        let file_type = entry.file_type();
        if file_type.as_ref().is_ok_and(|t| t.is_dir()) {
            let (files, scenarios) = tree(&path);
            return (acc.0 + files, acc.1 + scenarios);
        }
        if !file_type.is_ok_and(|t| t.is_file())
            || !path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("feature"))
        {
            return acc;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let scenarios = text
            .lines()
            .filter(|l| {
                let l = l.trim_start();
                l.starts_with("Scenario:") || l.starts_with("Scenario Outline:")
            })
            .count();
        (acc.0 + 1, acc.1 + scenarios)
    })
}
