//! The spec corpus tool (R-harness-01, R-harness-02): `id`, `new`, `count`,
//! `archive`, `--check`. Run from the repo root, as the gate rows in `AGENTS.md` do.
// Test code opts out of the two panic lints at the crate root (R-workspace-09).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

const SPEC_DIR: &str = "spec";
const CHANGES_DIR: &str = "changes";
const TEMPLATE_DIR: &str = "changes/_template";
const FEATURES_DIR: &str = "crates/tau-test/tests/features";
const AGENTS: &str = "AGENTS.md";
const REVIEW: &str = ".agents/skills/spec-review/SKILL.md";
const NEEDS: &str = "[NEEDS CLARIFICATION]";
const MARKERS: [&str; 3] = ["ADDED", "MODIFIED", "REMOVED"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str).unwrap_or("");
    let rest = if args.is_empty() {
        &args[..]
    } else {
        &args[1..]
    };
    let code = match first {
        "id" | "--id" => id_cmd(rest),
        "new" | "--new" => new_cmd(rest),
        "count" => count_cmd(),
        "archive" => archive_cmd(rest),
        "--check" => check_cmd(),
        "help" | "-h" | "--help" => {
            usage();
            0
        }
        other => {
            eprintln!("spec: unknown argument {other:?}");
            usage();
            1
        }
    };
    exit(code);
}

fn usage() {
    eprintln!(
        "usage: spec -- id <domain> | new <change-id> | count | archive <change-id> | --check"
    );
}

/// One `### R-<domain>-<NN>` clause, with the `## ` marker section it sits under.
struct Clause {
    path: String,
    line: usize,
    id: String,
    section: String,
    text: String,
    acceptance: String,
}

/// `R-<domain>-<NN>` splits at the last hyphen, so a hyphenated domain works.
fn split_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix("R-")?.rsplit_once('-')
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("md"))
}

fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            out.extend(markdown_files(&path));
        } else if is_markdown(&path) {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// A template copy is not a delta. `_template`, `_domain_` and `_area_` are the
/// rename-me markers `spec new` copies into a freshly opened change.
fn is_placeholder(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("_template") | Some("_domain_") | Some("_area_")
        )
    })
}

/// Corpus text, normalised to LF so a CRLF file cannot mix endings on rewrite.
/// An unreadable file is a finding, never an empty corpus.
fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map(|text| text.replace('\r', ""))
        .map_err(|e| format!("unreadable: {e}"))
}

fn clauses_in(path: &str, text: &str) -> Vec<Clause> {
    let lines: Vec<&str> = text.lines().collect();
    let fenced = fence_flags(&lines);
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if fenced[idx] {
            continue;
        }
        let Some(head) = clause_head(line) else {
            continue;
        };
        let id = head.split_whitespace().next().unwrap_or("");
        if !id.starts_with("R-") {
            continue;
        }
        let mut end = idx + 1;
        while end < lines.len()
            && !((lines[end].starts_with('#') || is_clause_head(lines[end])) && !fenced[end])
        {
            end += 1;
        }
        let section = lines[..idx]
            .iter()
            .enumerate()
            .rev()
            .find(|(i, l)| !fenced[*i] && l.starts_with("## "))
            .map(|(_, l)| (*l).to_string())
            .unwrap_or_default();
        let acceptance = lines[idx..end]
            .iter()
            .find_map(|l| l.strip_prefix("Acceptance:").map(|a| a.trim().to_string()))
            .unwrap_or_default();
        out.push(Clause {
            path: path.to_string(),
            line: idx + 1,
            id: id.to_string(),
            section,
            text: lines[idx..end].join("\n"),
            acceptance,
        });
    }
    out
}

/// The text after a clause heading's hashes. CommonMark's ATX heading allows a
/// space or a tab after `###`, so a tab form is a clause and both scans must see
/// it; a clause one scan misses escapes the ID, duplicate, cap, acceptance and
/// coverage checks, and hands its number back to `spec -- id`.
fn clause_head(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("###")?;
    if rest.starts_with(' ') || rest.starts_with('\t') {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// True when the line opens a clause, at any indent and after any tab.
fn is_clause_head(line: &str) -> bool {
    clause_head(line).is_some()
}

/// A line that names a clause ID next to its hashes, in a form no scan reads:
/// `###R-tools-01  Name` opens no clause, so the ID escapes the duplicate, cap and
/// acceptance checks and `spec -- id` hands the same number out again.
fn stray_heading(line: &str) -> Option<&'static str> {
    let rest = line.trim_start().strip_prefix("###")?;
    if is_clause_head(line) {
        return None;
    }
    rest.trim_start_matches('#')
        .trim_start()
        .starts_with("R-")
        .then_some("a clause heading has no space after its hashes, so no scan reads it")
}

/// The heading shape rules, read off the clause's own first line: `spec-authoring`
/// sets `### R-<domain>-<NN>  <name <=6 words>` at column 0. A clause the scanner
/// skipped would escape every other check, so an indented heading is reported, in
/// deltas too. The word cap is checked on the live corpus only: the delta sweep
/// that brings the open changes to it is a task of `change_spec_requirements`.
fn heading_problems(cap: bool, clause: &Clause) -> Vec<String> {
    let head = clause.text.lines().next().unwrap_or_default();
    let mut out = Vec::new();
    if head.starts_with(' ') || head.starts_with('\t') {
        out.push("a clause heading sits out of column 0".to_string());
    } else if !head.starts_with("### ") {
        out.push("a clause heading separates its hashes with no space".to_string());
    }
    let words = head.split_whitespace().skip(2).count();
    if cap && words > 6 {
        out.push(format!("heading name is {words} words, the cap is 6"));
    }
    out
}

/// True for the lines of a ``` fenced block, fences included: a `#` there is
/// example text, so it neither opens a clause nor ends one.
fn fence_flags(lines: &[&str]) -> Vec<bool> {
    let mut flags = vec![false; lines.len()];
    let mut inside = false;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            flags[i] = true;
            inside = !inside;
            continue;
        }
        flags[i] = inside;
    }
    flags
}

fn domain_names() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(SPEC_DIR) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out
}

/// Gate names, from the first column of the gate table in `AGENTS.md`.
fn gate_names() -> Vec<String> {
    let Ok(text) = fs::read_to_string(AGENTS) else {
        return Vec::new();
    };
    // Only the rows of the Gates table are gates; a row of any other table in the
    // file is not an acceptance target.
    let mut in_gates = false;
    text.lines()
        .filter_map(|l| {
            if let Some(title) = l.strip_prefix("## ") {
                in_gates = title.trim() == "Gates";
                return None;
            }
            in_gates
                .then_some(l)
                .filter(|l| l.starts_with("| ") && !l.starts_with("| Gate"))
                .filter(|l| !l.contains("---|"))
                .map(first_cell)
        })
        .filter(|n| !n.is_empty() && n.starts_with(|c: char| c.is_lowercase()))
        .collect()
}

fn first_cell(line: &str) -> String {
    line.trim_matches('|')
        .trim()
        .split('|')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Check numbers in the `spec-review` checklist table, e.g. `15`, `11b`.
fn review_checks(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("| "))
        .map(first_cell)
        .filter(|n| !n.is_empty() && n != "#" && !n.starts_with("---"))
        .filter(|n| {
            n.chars().next().is_some_and(|c| c.is_ascii_digit())
                && n.chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        })
        .collect()
}

/// The scenario tags in the feature tree, as whole tokens, without the `@`.
fn feature_tags() -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    collect_tags(Path::new(FEATURES_DIR), &mut tags);
    tags
}

fn collect_tags(dir: &Path, tags: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_tags(&path, tags);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("feature"))
        {
            let text = read_text(&path).unwrap_or_default();
            // Gherkin tags stand on their own line above the item they tag, so a
            // `@R-…` inside a step text or a comment is not a scenario tag.
            for line in text
                .lines()
                .filter(|l| l.trim_start().starts_with('@'))
                .flat_map(|l| l.split_whitespace())
            {
                if let Some(tag) = line.strip_prefix('@')
                    && tag.starts_with("R-")
                {
                    tags.insert(tag.to_string());
                }
            }
        }
    }
}

/// The delta files of open changes, and the requirement files under `spec/`.
fn corpus_files() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut spec_files = Vec::new();
    let mut delta_files = Vec::new();
    for path in markdown_files(Path::new(SPEC_DIR)) {
        spec_files.push(path);
    }
    for path in markdown_files(Path::new(CHANGES_DIR)) {
        let in_specs = path.components().any(|c| c.as_os_str() == "specs");
        let archived = path.components().any(|c| c.as_os_str() == "archive");
        if in_specs && !archived && !is_placeholder(&path) {
            delta_files.push(path);
        }
    }
    (spec_files, delta_files)
}

/// Fence-aware parsing is only sound when a file's ``` fences pair up; an odd count
/// means every later line is read as body, which a clause range would then swallow.
fn fences_balanced(text: &str) -> bool {
    text.lines()
        .filter(|l| l.trim_start().starts_with("```"))
        .count()
        % 2
        == 0
}

/// A removal keeps the separator blank line, which can stack with the one that
/// preceded the clause; MD012 forbids a blank run. Collapse outside fences to one
/// blank line and end the file with exactly one newline.
fn collapse_blank_runs(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let fenced = fence_flags(&lines);
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() && !fenced[i] && out.last().is_some_and(|p| p.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    format!("{}\n", out.join("\n").trim_end())
}
fn check_cmd() -> i32 {
    let domains = domain_names();
    let gates = gate_names();
    let checks = review_checks(&fs::read_to_string(REVIEW).unwrap_or_default());
    let tags = feature_tags();
    let (spec_files, delta_files) = corpus_files();
    // A linter that read nothing must not report a pass: these paths are relative
    // to the repo root, which is where the gate rows run.
    if domains.is_empty()
        || fs::read_dir(CHANGES_DIR).is_err()
        || !Path::new(FEATURES_DIR).is_dir()
        || !Path::new(AGENTS).is_file()
        || !Path::new(REVIEW).is_file()
    {
        eprintln!(
            "spec: not a repo root: no {SPEC_DIR}/, {CHANGES_DIR}/, {AGENTS} or {REVIEW} here"
        );
        return 1;
    }

    let mut spec_clauses: Vec<Clause> = Vec::new();
    let mut delta_clauses: Vec<Clause> = Vec::new();
    let mut problems = 0;
    for (files, clauses) in [
        (&spec_files, &mut spec_clauses),
        (&delta_files, &mut delta_clauses),
    ] {
        for path in files {
            match read_text(path) {
                Ok(text) => {
                    if !fences_balanced(&text) {
                        println!(
                            "{}: unclosed code fence, headings after it parse as body",
                            path.display()
                        );
                        problems += 1;
                    }
                    let lines: Vec<&str> = text.lines().collect();
                    let fenced = fence_flags(&lines);
                    for (i, line) in lines.iter().enumerate() {
                        if fenced[i] {
                            continue;
                        }
                        if let Some(msg) = stray_heading(line) {
                            println!("{}:{}: {msg}", path.display(), i + 1);
                            problems += 1;
                        }
                    }
                    clauses.extend(clauses_in(&path.display().to_string(), &text));
                }
                Err(e) => {
                    println!("{}: {e}", path.display());
                    problems += 1;
                }
            }
        }
    }

    let spec_ids: BTreeSet<&str> = spec_clauses.iter().map(|c| c.id.as_str()).collect();
    // R-harness-11: a withdrawal inside its own change keeps its number issued.
    let withdrawn_numbers = withdrawn_ids(&delta_clauses, &spec_ids);
    let mut seen_spec: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_delta: BTreeMap<&str, &str> = BTreeMap::new();
    let mut pending = 0;

    for clause in spec_clauses.iter() {
        let report = |msg: &str| {
            println!("{}:{}: {msg}", clause.path, clause.line);
            1
        };
        let Some((domain, number)) = split_id(&clause.id) else {
            problems += report("id is not R-<domain>-<NN>");
            continue;
        };
        if !domains.iter().any(|d| d == domain) {
            problems += report(&format!(
                "domain {domain} has no directory under {SPEC_DIR}"
            ));
        }
        if number.len() < 2 || number.len() > 4 || !number.bytes().all(|b| b.is_ascii_digit()) {
            problems += report("number is not 2..4 digits");
        }
        if let Some(first) = seen_spec.get(clause.id.as_str()) {
            problems += report(&format!("id already allocated in {first}"));
        } else {
            seen_spec.insert(&clause.id, &clause.path);
        }
        for msg in heading_problems(true, clause) {
            problems += report(&msg);
        }
        problems += check_acceptance(
            clause,
            &gates,
            &checks,
            &tags,
            true,
            &mut pending,
            FEATURES_DIR,
        );
    }

    for clause in delta_clauses.iter() {
        let report = |msg: &str| {
            println!("{}:{}: {msg}", clause.path, clause.line);
            1
        };
        let Some((domain, number)) = split_id(&clause.id) else {
            problems += report("id is not R-<domain>-<NN>");
            continue;
        };
        if !domains.iter().any(|d| d == domain) {
            problems += report(&format!(
                "domain {domain} has no directory under {SPEC_DIR}"
            ));
        }
        if number.len() < 2 || number.len() > 4 || !number.bytes().all(|b| b.is_ascii_digit()) {
            problems += report("number is not 2..4 digits");
        }
        if let Some(first) = seen_delta.get(clause.id.as_str()) {
            problems += report(&format!("id already allocated in {first}"));
        } else {
            seen_delta.insert(&clause.id, &clause.path);
        }
        // A withdrawal inside its own change has no acceptance ref: the rule is gone and
        // no scenario runs it (R-harness-11).
        let is_withdrawn = number.parse::<u32>().is_ok_and(|n| {
            withdrawn_numbers
                .get(domain)
                .is_some_and(|nums| nums.contains(&n))
        });
        // The heading shape binds a withdrawal too: its heading is scanned, so a
        // malformed one is reported even when its acceptance ref is waived.
        for msg in heading_problems(false, clause) {
            problems += report(&msg);
        }
        if !is_withdrawn {
            problems += check_acceptance(
                clause,
                &gates,
                &checks,
                &tags,
                false,
                &mut pending,
                FEATURES_DIR,
            );
        }

        if !MARKERS.iter().any(|m| clause.section.contains(m)) {
            problems += report("delta clause sits under no ADDED/MODIFIED/REMOVED marker");
        } else if clause.section.contains("MODIFIED") && !spec_ids.contains(clause.id.as_str()) {
            problems += report("MODIFIED names no requirement in spec/");
        } else if clause.section.contains("REMOVED")
            && !withdrawn(clause)
            && !spec_ids.contains(clause.id.as_str())
        {
            problems += report("REMOVED names no requirement in spec/");
        } else if clause.section.contains("ADDED") && spec_ids.contains(clause.id.as_str()) {
            problems += report("ADDED names a requirement spec/ already has");
        }
    }

    // R-harness-02: a number `id` never issued. `id` issues max+1, so every number
    // up to a domain's highest is live in the corpus or retired in changes/archive/.
    let clauses: Vec<&Clause> = spec_clauses.iter().chain(delta_clauses.iter()).collect();

    // A delta's `Acceptance: @R-…` names the scenario that runs the clause. While the
    // change is open a typo there is invisible, so require the id to name a clause.
    for clause in delta_clauses.iter() {
        for line in clause.text.lines() {
            let Some(rest) = line.trim().strip_prefix("Acceptance:") else {
                continue;
            };
            let Some(id) = rest
                .split_whitespace()
                .next()
                .and_then(|t| t.strip_prefix('@'))
            else {
                continue;
            };
            if !clauses.iter().any(|c| c.id == id) {
                println!(
                    "{}: {} accepts @{id}, which names no requirement in {SPEC_DIR}/ or any delta",
                    clause.path, clause.id
                );
                problems += 1;
            }
        }
    }
    let mut archived_findings = Vec::new();
    let archived = archived_numbers(&mut archived_findings);
    for finding in archived_findings {
        println!("spec: {finding}");
        problems += 1;
    }
    let mut gap_shown: BTreeMap<String, usize> = BTreeMap::new();
    for (domain, n) in allocation_gaps(&clauses, &archived) {
        let shown = gap_shown.entry(domain.clone()).or_insert(0);
        *shown += 1;
        if *shown <= 5 {
            println!(
                "spec/{domain}: R-{domain}-{n:02} is in no corpus file and in no archived delta, so `id` never issued it"
            );
        }
        problems += 1;
    }
    for (domain, shown) in gap_shown.iter().filter(|(_, c)| **c > 5) {
        println!("spec/{domain}: … and {} more unissued numbers", shown - 5);
    }

    // The reverse direction: a scenario tag naming no requirement is a typo that
    // leaves the requirement untested while the behaviour lane stays green.
    for tag in &tags {
        if !clauses.iter().any(|c| c.id == *tag) {
            println!("spec: scenario tag @{tag} names no requirement in {SPEC_DIR}/ or any delta");
            problems += 1;
        }
    }
    let mut needs = 0;
    for path in spec_files.iter().chain(delta_files.iter()) {
        let text = read_text(path).unwrap_or_default();
        let hits = text.matches(NEEDS).count();
        if hits > 0 {
            println!("{}: {hits} {NEEDS}", path.display());
            needs += hits;
        }
    }

    println!(
        "spec lint: {} clauses in spec/, {} in deltas; {} scenario refs pending archive; {} {NEEDS}",
        spec_clauses.len(),
        delta_clauses.len(),
        pending,
        needs
    );
    if problems > 0 {
        println!("spec lint: {problems} problem(s)");
        return 1;
    }
    0
}

/// The IDs of archived deltas. A retired requirement leaves `spec/` by design, and
/// its number stays issued, so it is not a hole. See `allocation_gaps`.
fn archived_numbers(findings: &mut Vec<String>) -> BTreeMap<String, BTreeSet<u32>> {
    let mut out: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for path in markdown_files(&Path::new(CHANGES_DIR).join("archive")) {
        // Only delta files carry clause ids; a `### R-x-99` heading in an archived
        // proposal or tasks.md is a citation, and must not retire a number.
        if !path.components().any(|c| c.as_os_str() == "specs") {
            continue;
        }
        let Ok(text) = read_text(&path) else {
            findings.push(format!(
                "{}: unreadable, its archived ids cannot be counted",
                path.display()
            ));
            continue;
        };
        for clause in clauses_in(&path.display().to_string(), &text) {
            if let Some((domain, number)) = split_id(&clause.id)
                && number.len() <= 4
                && let Ok(n) = number.parse::<u32>()
            {
                out.entry(domain.to_string()).or_default().insert(n);
            }
        }
    }
    out
}

/// A clause a change retires inside the change that added it: `## REMOVED` with no
/// `Supersedes:` line, so it retires nothing `spec/` holds. The rule never landed, so it
/// needs no acceptance ref, has no clause to delete at archive, and keeps its number
/// consumed so `id` never issues it again (R-harness-11). A `## REMOVED` clause that does
/// carry `Supersedes:` retires a rule `spec/` holds, and stays checked as before.
fn withdrawn(clause: &Clause) -> bool {
    clause.section.contains("REMOVED") && !clause.text.contains("Supersedes:")
}

/// A withdrawal, judged against the `spec/` file it applies to: `## REMOVED` with no
/// `Supersedes:` naming an id that file does not hold. The same shape naming an id the
/// file does hold retires a landed rule, and `apply_delta` deletes it (R-harness-11).
fn withdrawn_in(text: &str, clause: &Clause) -> bool {
    withdrawn(clause) && clause_range(text, &clause.id).is_none()
}

/// The numbers a change withdraws inside its own change, by domain.
fn withdrawn_ids(delta: &[Clause], spec_ids: &BTreeSet<&str>) -> BTreeMap<String, BTreeSet<u32>> {
    let mut numbers: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for clause in delta {
        if !withdrawn(clause) {
            continue;
        }
        let Some((domain, number)) = split_id(&clause.id) else {
            continue;
        };
        if spec_ids.contains(clause.id.as_str()) || number.len() > 4 {
            continue;
        }
        if let Ok(n) = number.parse::<u32>() {
            numbers.entry(domain.to_string()).or_default().insert(n);
        }
    }
    numbers
}

/// Numbers in `1..=highest` of a domain that are neither live nor archived: IDs no
/// `id` call issued, which is what R-harness-02 rejects.
fn allocation_gaps(
    live: &[&Clause],
    retired: &BTreeMap<String, BTreeSet<u32>>,
) -> Vec<(String, u32)> {
    let mut numbers: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for clause in live {
        // A number outside the format rule is a problem the caller reports; it must
        // not drive a scan of 1..=99999.
        if let Some((domain, number)) = split_id(&clause.id)
            && number.len() <= 4
            && let Ok(n) = number.parse::<u32>()
        {
            numbers.entry(domain.to_string()).or_default().insert(n);
        }
    }
    let domains: BTreeSet<&String> = numbers.keys().chain(retired.keys()).collect();
    let mut gaps = Vec::new();
    for domain in domains {
        let live = numbers.get(domain).cloned().unwrap_or_default();
        let retired = retired.get(domain).cloned().unwrap_or_default();
        let top = live.union(&retired).max().copied().unwrap_or(0);
        for n in 1..=top {
            if !live.contains(&n) && !retired.contains(&n) {
                gaps.push((domain.clone(), n));
            }
        }
    }
    gaps
}

/// Resolve one acceptance ref. A delta clause's scenario is written by the same
/// change, so its tag is counted pending until the delta lands in `spec/`.
fn check_acceptance(
    clause: &Clause,
    gates: &[String],
    checks: &[String],
    tags: &BTreeSet<String>,
    in_spec: bool,
    pending: &mut usize,
    features: &str,
) -> usize {
    let report = |msg: &str| {
        println!("{}:{}: {msg}", clause.path, clause.line);
        1
    };
    let value = clause.acceptance.trim();
    if value.is_empty() {
        return report("no Acceptance: ref");
    }
    if let Some(tag) = value.strip_prefix('@') {
        if tags.contains(tag) {
            return 0;
        }
        if in_spec {
            return report(&format!("@{tag} matches no scenario tag in {features}"));
        }
        *pending += 1;
        return 0;
    }
    if let Some(name) = value.strip_prefix("gate ") {
        if gates.iter().any(|g| g == name) {
            return 0;
        }
        return report(&format!("gate {name:?} is not a row in {AGENTS}"));
    }
    if let Some(check) = value.strip_prefix("spec-review check ") {
        if checks.iter().any(|c| c == check) {
            return 0;
        }
        return report(&format!("spec-review check {check} is not in {REVIEW}"));
    }
    report(&format!("unrecognised acceptance ref {value:?}"))
}

fn id_cmd(args: &[String]) -> i32 {
    let Some(domain) = args.first() else {
        eprintln!("spec: id needs a domain");
        return 1;
    };
    if !domain_names().iter().any(|d| d == domain) {
        eprintln!("spec: no domain {domain} under {SPEC_DIR}");
        return 1;
    }
    // An archived (retired) number was issued once, so `id` must never hand it out
    // again; an archived file it cannot read is a refusal, not an empty set.
    let mut findings = Vec::new();
    let archived = archived_numbers(&mut findings);
    if !findings.is_empty() {
        for finding in findings {
            eprintln!("spec: {finding}");
        }
        return 1;
    }
    let mut highest = archived
        .get(domain)
        .and_then(|nums| nums.iter().max().copied())
        .unwrap_or(0);
    let (spec_files, delta_files) = corpus_files();
    for path in spec_files.iter().chain(delta_files.iter()) {
        let Ok(text) = read_text(path) else {
            eprintln!(
                "spec: {}: unreadable, cannot allocate a safe ID",
                path.display()
            );
            return 1;
        };
        for clause in clauses_in(&path.display().to_string(), &text) {
            if let Some((d, n)) = split_id(&clause.id)
                && d == domain
                && n.len() <= 4
                && let Ok(n) = n.parse::<u32>()
            {
                highest = highest.max(n);
            }
        }
    }
    if highest >= 9999 {
        eprintln!("spec: {domain} is at the 4-digit cap, no id left to issue");
        return 1;
    }
    println!("R-{domain}-{:02}", highest + 1);
    0
}

fn count_cmd() -> i32 {
    // Outside the repo root there is no corpus to count, so 0 is a refusal.
    let domains = domain_names();
    if domains.is_empty() {
        eprintln!("count: not a repo root: no {SPEC_DIR}/ here");
        return 1;
    }
    let mut total = 0;
    for domain in domains {
        let mut n = 0;
        for path in markdown_files(Path::new(&format!("{SPEC_DIR}/{domain}"))) {
            let Ok(text) = read_text(&path) else {
                eprintln!("count: {}: unreadable", path.display());
                return 1;
            };
            n += clauses_in(&path.display().to_string(), &text).len();
        }
        println!("{domain}\t{n}");
        total += n;
    }
    println!("Corpus: {total} requirements");
    0
}

/// A change id is a bare snake_case name: never a path, so it cannot escape
/// `changes/` when `archive` renames it.
fn is_change_id(id: &str) -> bool {
    !id.is_empty()
        && id != "archive"
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !id.starts_with('_')
        && !id.contains("__")
}
fn new_cmd(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("spec: new needs a change id");
        return 1;
    };
    if !is_change_id(id) {
        eprintln!("spec: change id must be snake_case, got {id:?}");
        return 1;
    }
    let target = Path::new(CHANGES_DIR).join(id);
    if target.exists() {
        eprintln!("spec: changes/{id} already exists");
        return 1;
    }
    match copy_tree(Path::new(TEMPLATE_DIR), &target, id) {
        Ok(()) => {
            println!("created changes/{id}");
            0
        }
        Err(e) => {
            eprintln!("spec: {}", e.join("; "));
            1
        }
    }
}

fn copy_tree(from: &Path, to: &Path, id: &str) -> Result<(), Vec<String>> {
    copy_tree_inner(from, to, id).inspect_err(|_| {
        // A half-copied change makes every `spec new <id>` retry trip over
        // target.exists(), so undo the partial copy before reporting.
        let _ = fs::remove_dir_all(to);
    })
}

fn copy_tree_inner(from: &Path, to: &Path, id: &str) -> Result<(), Vec<String>> {
    // Read the source first, so a missing template leaves no empty changes/<id>/
    // behind that every retry then trips over.
    let entries = fs::read_dir(from).map_err(|e| vec![format!("{}: {e}", from.display())])?;
    fs::create_dir_all(to).map_err(|e| vec![format!("{}: {e}", to.display())])?;
    for entry in entries {
        let source = entry
            .map_err(|e| vec![format!("{}: {e}", from.display())])?
            .path();
        let Some(name) = source.file_name() else {
            continue;
        };
        let mut name = name.to_string_lossy().into_owned();
        if source.is_dir() {
            copy_tree(&source, &to.join(name), id)?;
            continue;
        }
        name = name.replace("{id}", id);
        let text = fs::read_to_string(&source)
            .map_err(|e| vec![format!("{}: {e}", source.display())])?
            .replace("{id}", id);
        fs::write(to.join(&name), text)
            .map_err(|e| vec![format!("{}: {e}", to.join(name).display())])?;
    }
    Ok(())
}

fn archive_cmd(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("spec: archive needs a change id");
        return 1;
    };
    if !is_change_id(id) {
        eprintln!("spec: change id must be snake_case, got {id:?}");
        return 1;
    }
    let change = Path::new(CHANGES_DIR).join(id);
    let specs = change.join("specs");

    let mut plan: Vec<(PathBuf, Vec<Clause>)> = Vec::new();
    for delta in markdown_files(&specs) {
        // `spec new` copies the templates into a fresh change; those copies are not
        // deltas, and applying one would write placeholder prose into spec/.
        if is_placeholder(&delta) {
            continue;
        }
        let Ok(text) = read_text(&delta) else {
            eprintln!("spec: {}: unreadable", delta.display());
            return 1;
        };
        let Ok(relative) = delta.strip_prefix(&specs) else {
            continue;
        };
        let target = Path::new(SPEC_DIR).join(relative);
        plan.push((target, clauses_in(&delta.display().to_string(), &text)));
    }
    if plan.is_empty() {
        eprintln!("spec: changes/{id}/specs holds no delta to apply");
        return 1;
    }

    // Refuse before touching anything: spec/ and changes/ stay as they were.
    let refusals = validate_plan(&plan);
    if !refusals.is_empty() {
        for refusal in refusals {
            eprintln!("spec: {refusal}");
        }
        return 1;
    }

    // Move first: the delta text survives in the archive, so a write failure after
    // the move is recovered by hand, never by a half-deleted change.
    let dest = Path::new(CHANGES_DIR)
        .join("archive")
        .join(format!("{}-{id}", today()));
    if dest.exists() {
        eprintln!("spec: {} exists", dest.display());
        return 1;
    }
    let moved = fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))
        .and_then(|()| fs::rename(&change, &dest));
    if let Err(e) = moved {
        eprintln!("spec: {e}");
        return 1;
    }

    // Compute every target in memory first: a refusal then writes nothing at all.
    let mut computed: Vec<(&PathBuf, String)> = Vec::new();
    let mut failures = Vec::new();
    for (target, clauses) in &plan {
        match current_text(target)
            .and_then(|text| apply_delta(&text, clauses).map_err(|m| m.join("; ")))
        {
            Ok(text) => computed.push((target, text)),
            Err(message) => failures.push(message),
        }
    }
    if !failures.is_empty() {
        for failure in failures {
            eprintln!("spec: {failure}");
        }
        eprintln!(
            "spec: the change is at {}; no spec/ file was written",
            dest.display()
        );
        return 1;
    }

    let total = computed.len();
    let mut applied = 0;
    for (target, text) in computed {
        if let Err(messages) = write_target(target, &text) {
            for message in messages {
                eprintln!("spec: {message}");
            }
            eprintln!(
                "spec: the change is at {}; {applied} of {total} spec/ files were written",
                dest.display()
            );
            return 1;
        }
        applied += 1;
    }
    println!("archived changes/{id} to {}", dest.display());
    println!("applied {applied} delta file(s); update spec/index.md counts and links");
    0
}

fn validate_plan(plan: &[(PathBuf, Vec<Clause>)]) -> Vec<String> {
    let mut refusals = Vec::new();
    for (target, clauses) in plan {
        // The same read the write path uses: a missing target is the new-file case, and
        // any other read error is a refusal, not an "empty file" that answers the
        // MODIFIED/REMOVED probes with "no clause here" and lets a write overwrite it.
        let text = match current_text(target) {
            Ok(text) => text,
            Err(message) => {
                refusals.push(message);
                continue;
            }
        };
        for clause in clauses {
            // ADDED and MODIFIED headings land in the live corpus, where the lint counts
            // them, so archiving an over-cap heading would turn `spec lint` red: refuse.
            if clause.section.contains("ADDED") || clause.section.contains("MODIFIED") {
                // A clause appends at the file's end; an unbalanced fence in its own text
                // leaves the file fenced, and every clause after it is example text.
                if !fences_balanced(&clause.text) {
                    refusals.push(format!(
                        "{}: clause {} carries an unclosed code fence",
                        clause.path, clause.id
                    ));
                }
                for msg in heading_problems(true, clause) {
                    refusals.push(format!(
                        "{}: clause {} heading: {msg}",
                        clause.path, clause.id
                    ));
                }
            }
            if !MARKERS.iter().any(|m| clause.section.contains(m)) {
                refusals.push(format!(
                    "{}: clause {} sits under no ADDED/MODIFIED/REMOVED marker",
                    clause.path, clause.id
                ));
            } else if (clause.section.contains("MODIFIED")
                || (clause.section.contains("REMOVED") && !withdrawn(clause)))
                && clause_range(&text, &clause.id).is_none()
            {
                refusals.push(format!(
                    "{}: no clause {} to apply",
                    target.display(),
                    clause.id
                ));
            } else if clause.section.contains("ADDED") && clause_range(&text, &clause.id).is_some()
            {
                // Appending an id the corpus already has would duplicate it and consume
                // the delta, so refuse before the change moves to the archive.
                refusals.push(format!(
                    "{}: {} already exists, ADDED would duplicate it",
                    target.display(),
                    clause.id
                ));
            }
        }
    }
    refusals
}

/// The text a delta applies to. A missing target is the new-file case; any other
/// read error is a refusal, never an answer of "empty file" that overwrites it.
fn current_text(target: &Path) -> Result<String, String> {
    match fs::read_to_string(target) {
        Ok(text) => Ok(text.replace('\r', "")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let area = target
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("domain");
            Ok(format!("# {area} index\n"))
        }
        Err(e) => Err(format!("{}: unreadable: {e}", target.display())),
    }
}

fn write_target(target: &Path, text: &str) -> Result<(), Vec<String>> {
    if let Some(parent) = target.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        return Err(vec![e.to_string()]);
    }
    // Write-then-rename: a failed write leaves the previous file in place.
    let tmp = target.with_extension("md.tmp");
    if let Err(e) = fs::write(&tmp, text) {
        let _ = fs::remove_file(&tmp);
        return Err(vec![format!("{}: {e}", tmp.display())]);
    }
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        vec![format!("{}: {e}", target.display())]
    })
}

/// Append ADDED clauses, replace MODIFIED, delete REMOVED. The text is built in
/// memory, so a refusal never writes a partial file.
fn apply_delta(text: &str, clauses: &[Clause]) -> Result<String, Vec<String>> {
    let mut text = text.to_string();
    let mut added = Vec::new();
    let mut refusals = Vec::new();
    for clause in clauses {
        if clause.section.contains("ADDED") {
            added.push(clause.text.clone());
            continue;
        }
        if withdrawn_in(&text, clause) {
            // The rule never landed, so `spec/` has no clause to delete (R-harness-11).
            continue;
        }
        let Some(range) = clause_range(&text, &clause.id) else {
            refusals.push(format!("no clause {} to apply", clause.id));
            continue;
        };
        if clause.section.contains("MODIFIED") {
            text.replace_range(range, &format!("{}\n\n", clause.text.trim_end()));
        } else if clause.section.contains("REMOVED") {
            text.replace_range(range, "\n");
        } else {
            refusals.push(format!(
                "{}: clause {} has no marker",
                clause.path, clause.id
            ));
        }
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }
    if !added.is_empty() {
        if !last_heading_is_requirements(&text) {
            text = format!("{}\n\n## Requirements\n", text.trim_end());
        }
        for clause in added {
            text = format!("{}\n\n{}\n", text.trim_end(), clause.trim_end());
        }
    }
    // A removal keeps the separator blank line, which can stack with the one that
    // preceded the clause; MD012 forbids a blank run, so collapse 3+ newlines and
    // end the file with exactly one newline.
    text = collapse_blank_runs(&text);
    Ok(text)
}

/// Clauses append at the end of the file, so the last `## ` heading must be the
/// Requirements section or an ADDED clause lands inside some other section.
fn last_heading_is_requirements(text: &str) -> bool {
    // Fence-aware, like every other heading scan in this file: a fenced
    // `## Requirements` in an example is not the section a clause appends to.
    let lines: Vec<&str> = text.lines().collect();
    let fenced = fence_flags(&lines);
    lines
        .iter()
        .zip(fenced)
        .rev()
        .find(|(l, f)| !f && l.starts_with("## "))
        .is_some_and(|(l, _)| l.trim_end() == "## Requirements")
}

/// The byte range of one clause: its heading line, matched on the whole id
/// token, through the lines before the next heading.
fn clause_range(text: &str, id: &str) -> Option<Range<usize>> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let fenced = fence_flags(&lines);
    let mut start = 0;
    for (idx, line) in lines.iter().enumerate() {
        if !fenced[idx]
            && clause_head(line).and_then(|head| head.split_whitespace().next()) == Some(id)
        {
            let mut end = text.len();
            let mut offset = start + line.len();
            for (j, next) in lines[idx + 1..].iter().enumerate() {
                if !fenced[idx + 1 + j] && (next.starts_with('#') || is_clause_head(next)) {
                    end = offset;
                    break;
                }
                offset += next.len();
            }
            return Some(start..end);
        }
        start += line.len();
    }
    None
}

/// The archive directory name carries today's date, with no date dependency.
fn today() -> String {
    fmt_days(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 86_400)
            .unwrap_or(0) as i64,
    )
}

/// Days since the epoch to `YYYY-MM-DD`.
fn fmt_days(days: i64) -> String {
    let (mut y, mut m) = (1970, 1);
    let mut d = days;
    loop {
        let len = year_len(y);
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    const MONTHS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = year_len(y) == 366;
    for len in MONTHS {
        let len = if m == 2 && leap { 29 } else { len };
        if d < len {
            break;
        }
        d -= len;
        m += 1;
    }
    format!("{y:04}-{m:02}-{:02}", d + 1)
}

fn year_len(y: i64) -> i64 {
    if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
        366
    } else {
        365
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(id: &str, section: &str, text: &str) -> Clause {
        Clause {
            path: "delta".to_string(),
            line: 1,
            id: id.to_string(),
            section: section.to_string(),
            text: text.to_string(),
            acceptance: "gate workspace layout".to_string(),
        }
    }

    #[test]
    fn a_heading_out_of_column_zero_is_reported() {
        let ok = clause(
            "R-tools-01",
            "## Requirements",
            "### R-tools-01  Six word name here ok",
        );
        assert!(heading_problems(true, &ok).is_empty());
        let long = clause(
            "R-tools-02",
            "## Requirements",
            "### R-tools-02  seven word name over the cap now",
        );
        assert_eq!(
            heading_problems(true, &long),
            ["heading name is 7 words, the cap is 6"]
        );
        let indented = clause(
            "R-tools-03",
            "## Requirements",
            "    ### R-tools-03  Indented",
        );
        assert_eq!(
            heading_problems(true, &indented),
            ["a clause heading sits out of column 0"]
        );
    }

    #[test]
    fn id_splits_at_the_last_hyphen() {
        assert_eq!(split_id("R-domain-model-07"), Some(("domain-model", "07")));
        assert_eq!(split_id("R-workspace-01"), Some(("workspace", "01")));
        assert_eq!(split_id("workspace-01"), None);
    }

    #[test]
    fn date_arithmetic() {
        assert_eq!(fmt_days(0), "1970-01-01");
        assert_eq!(fmt_days(19782), "2024-02-29");
        assert_eq!(fmt_days(19783), "2024-03-01");
        // The last day of a leap year: the year loop must not go negative.
        assert_eq!(fmt_days(20088), "2024-12-31");
        assert_eq!(fmt_days(20089), "2025-01-01");
        assert_eq!(fmt_days(11017), "2000-03-01");
        assert_eq!(fmt_days(47540), "2100-02-28");
        assert_eq!(fmt_days(47541), "2100-03-01");
        assert_eq!(fmt_days(20770), "2026-11-13");
    }

    #[test]
    fn clause_range_matches_the_whole_id_token() {
        let text = "### R-workspace-010  Ten\nten body\n\n### R-workspace-01  One\none body\n";
        let range = clause_range(text, "R-workspace-01").unwrap();
        assert_eq!(&text[range], "### R-workspace-01  One\none body\n");
    }

    #[test]
    fn a_removed_middle_clause_keeps_the_blank_line_before_the_next_heading() {
        let text = "## Requirements\n\n### R-workspace-01  One\nbody one\nAcceptance: gate workspace layout\n\n### R-workspace-02  Two\nbody two\nAcceptance: gate workspace layout\n\n### R-workspace-03  Three\nbody three\nAcceptance: gate workspace layout\n";
        let deltas = [clause(
            "R-workspace-02",
            "## REMOVED Requirements",
            "### R-workspace-02  Two — Supersedes: R-workspace-02 → R-workspace-04",
        )];
        let out = apply_delta(text, &deltas).unwrap();
        assert!(!out.contains("### R-workspace-02"));
        assert!(
            out.contains("Acceptance: gate workspace layout\n\n### R-workspace-03"),
            "the next heading needs its blank line: {out}"
        );
        assert!(!out.contains("\n\n\n"), "no blank run, MD012: {out}");
        assert!(out.ends_with("body three\nAcceptance: gate workspace layout\n"));
    }
    #[test]
    fn apply_delta_modifies_removes_and_appends() {
        let text = "# workspace index\n\n### R-workspace-01  Flat members\nold clause\nAcceptance: gate workspace layout\n\n### R-workspace-010  Ten\nten body\nAcceptance: gate workspace layout\n\n### R-workspace-02  Named\nkeep\nAcceptance: gate workspace layout\n";
        let deltas = [
            clause(
                "R-workspace-01",
                "## MODIFIED Requirements",
                "### R-workspace-01  Flat members\nnew clause\nAcceptance: gate workspace layout",
            ),
            clause(
                "R-workspace-03",
                "## ADDED Requirements",
                "### R-workspace-03  Pinned\nnew rule\nAcceptance: gate toolchain pinned",
            ),
            clause(
                "R-workspace-02",
                "## REMOVED Requirements",
                "### R-workspace-02  Named — Supersedes: R-workspace-02 → R-workspace-04",
            ),
        ];
        let out = apply_delta(text, &deltas).unwrap();
        assert!(out.contains("new clause") && !out.contains("old clause"));
        assert!(out.contains("ten body"), "R-workspace-010 must survive");
        assert!(out.contains("### R-workspace-03  Pinned"));
        assert!(!out.contains("### R-workspace-02  Named"));
    }

    #[test]
    fn apply_delta_appends_under_a_requirements_heading() {
        let text = "# tools index\n\nOwns: crates/tau-core/tools.\n\n## Notes\n\nA note.\n";
        let deltas = [clause(
            "R-tools-01",
            "## ADDED Requirements",
            "### R-tools-01  Skills\nTHE SYSTEM SHALL load skills.\nAcceptance: @R-tools-01",
        )];
        let out = apply_delta(text, &deltas).unwrap();
        assert!(out.ends_with("Acceptance: @R-tools-01\n"));
        assert!(out.matches("## Requirements").count() == 1);
        assert!(out.find("## Requirements") < out.find("### R-tools-01"));
    }

    #[test]
    fn apply_delta_refuses_a_clause_it_cannot_find() {
        let text = "# harness index\n";
        let deltas = [clause(
            "R-harness-04",
            "## MODIFIED Requirements",
            "### R-harness-04  Lanes",
        )];
        assert_eq!(apply_delta(text, &deltas).unwrap_err().len(), 1);
    }

    #[test]
    fn apply_delta_refuses_an_unmarked_clause() {
        let text = "# harness index\n\n## Requirements\n\n### R-harness-04  Lanes\n\n- Proved by: gate unit + surface\n";
        let deltas = [clause(
            "R-harness-04",
            "## Unmarked",
            "### R-harness-04  Lanes\n\n- Proved by: gate unit + surface\n",
        )];
        let refusals = apply_delta(text, &deltas).unwrap_err();
        assert!(
            refusals[0].contains("R-harness-04 has no marker"),
            "{refusals:?}"
        );
    }

    #[test]
    fn a_heading_inside_a_code_fence_is_not_a_clause() {
        let text = "## ADDED Requirements\n\n### R-tools-01  Tools\n\n- Proved by: gate lints\n\n```toml\n### R-tools-02  Not a clause\n# a comment\n```\n";
        let ids: Vec<String> = clauses_in("x.md", text)
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, vec!["R-tools-01"]);
        // The clause runs past the fenced `#` line, to the end of the file.
        assert_eq!(clause_range(text, "R-tools-01").unwrap().end, text.len());
    }

    #[test]
    fn a_withdrawn_clause_keeps_its_number_issued() {
        let clauses: Vec<Clause> = [
            clause("R-session-01", "## ADDED Requirements", ""),
            clause("R-session-02", "## REMOVED Requirements", "Why: it landed."),
            clause("R-session-03", "## ADDED Requirements", ""),
            clause(
                "R-session-04",
                "## REMOVED Requirements",
                "Supersedes: R-session-04 → R-session-05",
            ),
        ]
        .into_iter()
        .collect();
        let retired = withdrawn_ids(&clauses, &BTreeSet::new());
        assert_eq!(retired.get("session").map(|n| n.len()), Some(1));
        // A `## REMOVED` clause carrying `Supersedes:` retires a rule `spec/` holds.
        assert!(!retired.get("session").is_some_and(|n| n.contains(&4)));
        let live: Vec<&Clause> = clauses.iter().filter(|c| !withdrawn(c)).collect();
        // 2 is a hole in the numbers that reach `spec/`, so the scan flags it on its own;
        // the withdrawal counts as issued, and the delta needs no successor.
        // Production consumes it the same way: `id_cmd` issues max+1 over `spec/` and
        // every open delta, so a withdrawn number is never handed out again.
        assert_eq!(
            allocation_gaps(&live, &BTreeMap::new())
                .into_iter()
                .map(|(_, n)| n)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(allocation_gaps(&live, &retired).is_empty());
    }

    #[test]
    fn archive_refuses_a_clause_whose_fence_never_closes() {
        let plan = [(
            PathBuf::from("spec/session/never-exists/spec.md"),
            vec![clause(
                "R-session-02",
                "## ADDED Requirements",
                "### R-session-02  A name\nTHE SYSTEM SHALL x.\n```\nexample",
            )],
        )];
        let refusals = validate_plan(&plan);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].contains("unclosed code fence"), "{refusals:?}");
        // A closed pair is the shape a clause carries an example in.
        let ok = [(
            PathBuf::from("spec/session/never-exists/spec.md"),
            vec![clause(
                "R-session-02",
                "## ADDED Requirements",
                "### R-session-02  A name\nTHE SYSTEM SHALL x.\n```\nexample\n```",
            )],
        )];
        assert_eq!(validate_plan(&ok), Vec::<String>::new());
    }

    #[test]
    fn a_heading_with_no_space_after_the_hashes_is_stray() {
        assert_eq!(
            stray_heading("###R-tools-01  Name"),
            Some("a clause heading has no space after its hashes, so no scan reads it")
        );
        assert_eq!(
            stray_heading("    ####R-tools-01  Name"),
            Some("a clause heading has no space after its hashes, so no scan reads it")
        );
        // The forms a scan reads: a space, a tab, prose, and a heading with no id.
        assert_eq!(stray_heading("### R-tools-01  Name"), None);
        assert_eq!(stray_heading("###\tR-tools-01  Name"), None);
        assert_eq!(stray_heading("### notes"), None);
    }

    #[test]
    fn archive_refuses_a_heading_the_live_lint_would_redden() {
        // A heading over the cap that lands in `spec/` turns `spec lint` red in the
        // archive commit, so the archive refuses before it writes anything.
        let plan = [(
            PathBuf::from("spec/session/never-exists/spec.md"),
            vec![clause(
                "R-session-02",
                "## ADDED Requirements",
                "### R-session-02  A heading with eight words in its name here\nTHE SYSTEM SHALL x.",
            )],
        )];
        let refusals = validate_plan(&plan);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].contains("the cap is 6"), "{refusals:?}");
    }

    #[test]
    fn a_withdrawn_clause_needs_nothing_in_spec_at_archive() {
        let plan = [(
            PathBuf::from("spec/session/never-exists/spec.md"),
            vec![
                clause("R-session-02", "## REMOVED Requirements", "Why: it landed."),
                clause(
                    "R-session-03",
                    "## MODIFIED Requirements",
                    "### R-session-03  A name\nWHEN x THE SYSTEM SHALL y.",
                ),
                clause(
                    "R-session-04",
                    "## REMOVED Requirements",
                    "Supersedes: R-session-04 → R-session-05",
                ),
            ],
        )];
        let refusals = validate_plan(&plan);
        // The withdrawal has no clause in `spec/` to delete. The retirement and the
        // modification each name a rule the target file has to hold.
        assert_eq!(refusals.len(), 2, "{refusals:?}");
        assert!(refusals.iter().any(|r| r.contains("R-session-03")));
        assert!(refusals.iter().any(|r| r.contains("R-session-04")));
        let applied = apply_delta(
            "",
            &[clause(
                "R-session-02",
                "## REMOVED Requirements",
                "Why: it landed.",
            )],
        )
        .expect("a withdrawal has nothing to apply");
        assert!(
            !applied.contains("R-session-02"),
            "a withdrawal writes nothing"
        );
        let retired = apply_delta(
            "",
            &[clause(
                "R-session-04",
                "## REMOVED Requirements",
                "Supersedes: R-session-04 → R-session-05",
            )],
        );
        assert!(
            retired.is_err_and(|r| r.iter().any(|f| f.contains("R-session-04"))),
            "a retirement whose rule `spec/` holds is refused, not skipped"
        );
    }

    #[test]
    fn a_removed_clause_naming_a_landed_rule_is_deleted_not_skipped() {
        // R-harness-11's precondition is part of the shape: a withdrawal is the clause
        // `spec/` never held. The same shape naming a landed rule is a retirement, and
        // archive deletes that rule rather than skipping over it.
        let landed = "## Requirements\n\n### R-session-05  Log rows are append-only\n\nTHE SYSTEM SHALL x.\n\n### R-session-06  Reads are short\n\nTHE SYSTEM SHALL y.\n";
        let removed = clause("R-session-05", "## REMOVED Requirements", "Why: it moved.");
        assert!(
            !withdrawn_in(landed, &removed),
            "a REMOVED clause naming a landed rule is not a withdrawal"
        );
        let applied = apply_delta(landed, &[removed]).expect("the removal applies");
        assert!(!applied.contains("R-session-05"), "the landed rule goes");
        assert!(applied.contains("R-session-06"), "its neighbour survives");
    }

    #[test]
    fn allocation_gaps_finds_a_hand_picked_number() {
        let clauses: Vec<Clause> = ["R-tools-01", "R-tools-02", "R-tools-04"]
            .iter()
            .map(|id| clause(id, "## ADDED Requirements", ""))
            .collect();
        let refs: Vec<&Clause> = clauses.iter().collect();
        assert_eq!(
            allocation_gaps(&refs, &BTreeMap::new()),
            vec![("tools".to_string(), 3)]
        );
        let contiguous: Vec<Clause> = ["R-tools-01", "R-tools-02", "R-tools-03"]
            .iter()
            .map(|id| clause(id, "## ADDED Requirements", ""))
            .collect();
        assert!(
            allocation_gaps(&contiguous.iter().collect::<Vec<_>>(), &BTreeMap::new()).is_empty()
        );
        // Archiving a REMOVED clause leaves a hole in spec/ on purpose: the archived
        // delta shows the number was issued, so it is not a gap.
        let mut retired: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        retired.insert("tools".to_string(), [3].into_iter().collect());
        assert!(allocation_gaps(&refs, &retired).is_empty());
    }

    #[test]
    fn an_over_long_number_is_not_scanned_as_gaps() {
        // A clause typed `### R-tools-99999` is a format problem, not 99,998 gap lines.
        let clauses: Vec<Clause> = ["R-tools-01", "R-tools-99999"]
            .iter()
            .map(|id| clause(id, "## ADDED Requirements", ""))
            .collect();
        let refs: Vec<&Clause> = clauses.iter().collect();
        assert!(allocation_gaps(&refs, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn clause_text_and_section_come_from_the_file() {
        let text = "## ADDED Requirements\n\n### R-workspace-01  Flat members\nTHE SYSTEM SHALL list members.\nAcceptance: gate workspace layout\n\n## Notes\n\nA note.\n";
        let clauses = clauses_in("changes/x/specs/workspace/spec.md", text);
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].section, "## ADDED Requirements");
        assert_eq!(clauses[0].acceptance, "gate workspace layout");
        assert!(clauses[0].text.contains("THE SYSTEM SHALL list members."));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    // The shapes a spec file holds: section headings, clause headings, fences,
    // prose, and indented prose that must not read as a heading.
    const SHAPES: [&str; 11] = [
        "## Requirements",
        "## Notes",
        "### R-a-01  one",
        "### R-a-01  one again",
        "### R-b-02  two",
        "### not-an-id",
        "```",
        "# index",
        "    ### R-a-03  indented",
        "###\tR-a-04  tabbed",
        "THE SYSTEM SHALL list members.",
    ];

    fn doc() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![
                proptest::sample::select(
                    SHAPES
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<String>>()
                ),
                "[a-z ]{0,12}",
                "[a-z ]{0,12}".prop_map(|s| format!("  {s}")),
            ],
            0..24,
        )
        .prop_map(|lines| lines.join("\n"))
    }

    fn removed(id: &str) -> Clause {
        Clause {
            path: "delta".to_string(),
            line: 1,
            id: id.to_string(),
            section: "## REMOVED Requirements".to_string(),
            text: format!("Supersedes: {id} → R-tools-99"),
            acceptance: String::new(),
        }
    }

    /// The clause IDs, sorted, so a multiset comparison is an equality.
    fn ids(text: &str) -> Vec<String> {
        let mut found: Vec<String> = clauses_in("t.md", text)
            .iter()
            .map(|c| c.id.clone())
            .collect();
        found.sort();
        found
    }

    proptest! {
        #[test]
        fn both_scans_find_the_same_clauses(ref text in doc()) {
            for clause in clauses_in("t.md", text) {
                prop_assert!(
                    clause_range(text, &clause.id).is_some(),
                    "clause_range lost {} that clauses_in found",
                    clause.id
                );
            }
            for id in ["R-a-01", "R-a-012", "R-a-03", "R-b-02"] {
                if clause_range(text, id).is_some() {
                    prop_assert!(
                        ids(text).contains(&id.to_string()),
                        "clauses_in lost {} that clause_range found",
                        id
                    );
                }
            }
        }

        #[test]
        fn removing_a_clause_leaves_the_others(ref text in doc()) {
            let before = ids(text);
            let Some(id) = before.first().cloned() else {
                return Ok(());
            };
            let result = apply_delta(text, &[removed(&id)]);
            prop_assert!(result.is_ok(), "refused: {:?}", result.unwrap_err());
            let mut want = before;
            want.remove(0);
            let after = result.unwrap();
            prop_assert_eq!(ids(&after), want);
        }

        #[test]
        fn blank_run_collapse_is_idempotent(ref text in doc()) {
            let once = collapse_blank_runs(text);
            prop_assert_eq!(once.clone(), collapse_blank_runs(&once));
            // MD012's blank run is the one outside a fence; a fenced example keeps
            // its own blank lines, so mask fenced lines before counting.
            let lines: Vec<&str> = once.lines().collect();
            let fenced = fence_flags(&lines);
            let mut prev_blank = false;
            for (i, line) in lines.iter().enumerate() {
                let blank = line.trim().is_empty() && !fenced[i];
                prop_assert!(!(blank && prev_blank), "blank run at line {i} of {once:?}");
                prev_blank = blank;
            }
        }
    }
}
