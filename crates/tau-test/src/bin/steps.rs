//! Generates `tests/features/STEPS.md` from the step definitions in `src/steps/`
//! (R-harness-03). `--check` compares instead of writing, so a scenario cannot
//! cite a step the runner never defined.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

const HEADER: &str = "# Step vocabulary

The allowed step vocabulary: a scenario uses only steps listed here, and adding a step is a code change listed in `tasks.md`. Generated from the step definitions in `crates/tau-test/src/steps/`: do not edit by hand.";

// shortcut: the vocabulary scans src/steps only, the layout the scaffold sets; a
// step defined elsewhere is runnable but unlisted. Widen the scan if definitions move.
const STEPS_DIR: &str = "src/steps";
const OUT: &str = "tests/features/STEPS.md";

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let steps_dir = root.join(STEPS_DIR);
    // A missing steps directory is a finding, not an empty vocabulary to write out.
    if !steps_dir.is_dir() {
        eprintln!("steps: {} is missing", steps_dir.display());
        exit(1);
    }
    let mut findings = Vec::new();
    let text = generate(&steps_dir, &mut findings);
    // A vocabulary that is missing a definition is never current, and never written.
    if !findings.is_empty() {
        for finding in &findings {
            eprintln!("steps: {finding}");
        }
        exit(1);
    }
    let out = root.join(OUT);
    let body = format!("{HEADER}\n\n{text}");

    if check {
        // A missing file is stale, not "matches the empty body".
        let Ok(current) = fs::read_to_string(&out) else {
            println!("{OUT} is missing; run: cargo run -p tau-test --bin steps");
            exit(1);
        };
        if current.trim_end() != body.trim_end() {
            println!("STEPS.md is stale; run: cargo run -p tau-test --bin steps");
            exit(1);
        }
        println!("steps current");
        return;
    }
    // Write-then-rename: a failed write leaves the previous vocabulary readable.
    let tmp = out.with_extension("md.tmp");
    let written = fs::write(&tmp, format!("{body}\n")).and_then(|()| fs::rename(&tmp, &out));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        eprintln!("steps: {e}");
        exit(1);
    }
    println!("wrote {}", out.display());
}

/// One line per definition: the keyword and its step text, in file order.
fn generate(dir: &Path, findings: &mut Vec<String>) -> String {
    let mut lines = Vec::new();
    for path in sources(dir, findings) {
        let Ok(text) = fs::read_to_string(&path) else {
            findings.push(format!(
                "{}: unreadable, its step definitions are missing from the vocabulary",
                path.display()
            ));
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            for keyword in ["given", "when", "then"] {
                let Some(rest) = line.strip_prefix(&format!("#[{keyword}(")) else {
                    continue;
                };
                // `expr = "…"`, `regex = "…"` and a bare `"…"` all carry the step text.
                if let Some(step) = quoted(rest) {
                    lines.push(format!("- {} {step}", capitalize(keyword)));
                } else {
                    findings.push(format!(
                        "{}: {} has no string literal, so its step text is not in the vocabulary",
                        path.display(),
                        line.split('(').next().unwrap_or(line)
                    ));
                }
            }
        }
    }
    if lines.is_empty() {
        return "No steps defined yet.".to_string();
    }
    lines.join("\n")
}

// shortcut: the first string literal of the attribute is the step text; an escaped
// quote inside one would truncate it, and a `regex = "…"` attribute contributes its
// pattern, so that vocabulary line is the regex. Upgrade when a step uses either.
fn quoted(text: &str) -> Option<String> {
    let (_, rest) = text.split_once('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    let first = chars.next().unwrap_or_default();
    first.to_uppercase().collect::<String>() + chars.as_str()
}

fn sources(dir: &Path, findings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        // An unreadable directory is a gap in the vocabulary, not an empty one.
        findings.push(format!(
            "{}: unreadable directory, its step definitions are missing",
            dir.display()
        ));
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            out.extend(sources(&path, findings));
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rs"))
        {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::{capitalize, quoted};

    #[test]
    fn step_text_comes_from_the_first_literal() {
        assert_eq!(
            quoted("\"the git index\")").as_deref(),
            Some("the git index")
        );
        assert_eq!(
            quoted("expr = \"the crates are read\")").as_deref(),
            Some("the crates are read")
        );
        assert_eq!(quoted("regex = \"^a step$\")").as_deref(), Some("^a step$"));
        assert_eq!(quoted("no literal"), None);
        assert_eq!(capitalize("given"), "Given".to_string());
    }
}
