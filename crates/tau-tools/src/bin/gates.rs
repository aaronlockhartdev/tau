//! The gate lane: run every `run` row of the `AGENTS.md` §Gates table, so the
//! table is the only place a gate is defined. Rows in state `M0` and `manual`
//! report skipped, never a fake pass. Run from the repo root:
//! `cargo run -p tau-tools --bin gates`.
// Test code opts out of the two panic lints at the crate root (R-workspace-09).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
use std::fs;
use std::io::Write;
use std::process::Output;
use std::process::exit;

const AGENTS: &str = "AGENTS.md";
const THIS_LANE: &str = "--bin gates";
const STATES: [&str; 3] = ["run", "M0", "manual"];

/// A row of the gate table.
struct Gate {
    name: String,
    command: String,
    state: String,
}

/// Split a markdown table row into cells. A `\|` inside a cell is an escaped
/// pipe, kept as a literal `|`.
fn cells(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            cell.push(if c == '|' { '|' } else { c });
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '|' => out.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    out.push(cell);
    // A markdown row's leading and trailing pipes yield empty outer cells.
    if line.starts_with('|') {
        out.remove(0);
    }
    if line.ends_with('|') {
        out.pop();
    }
    out.iter().map(|s| s.trim().to_string()).collect()
}

/// A row's command is the first backticked span of its command cell; text
/// around it is prose.
fn first_backtick(cell: &str) -> String {
    let mut parts = cell.split('`');
    parts.next();
    parts.next().unwrap_or("").to_string()
}

/// The rows of the §Gates table: from the `## Gates` heading to the next
/// heading. The header row and the `|---|` separator are not gates: their state
/// is not a known state.
/// The rows of the §Gates table: from the `## Gates` heading to the next
/// heading. A row the lane cannot run — wrong cell count, a state that is not
/// one of the three — comes back as a finding, so a gate is never dropped
/// silently. The header row and the `|---|` separator are not gate rows.
fn gates(md: &str) -> (Vec<Gate>, Vec<String>) {
    let mut rows: Vec<Gate> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    let mut inside = false;
    for line in md.lines() {
        if line.starts_with('#') {
            inside = line.trim_start_matches('#').trim() == "Gates";
            continue;
        }
        if !inside || !line.trim_start().starts_with('|') {
            continue;
        }
        let cols = cells(line.trim());
        if cols.len() != 3 {
            bad.push(format!(
                "{} cells, need 3 (a `|` inside a cell is escaped `\\|`): {line}",
                cols.len()
            ));
            continue;
        }
        if cols[0] == "Gate" || cols.iter().all(|c| c.starts_with('-')) {
            continue;
        }
        if !STATES.contains(&cols[2].as_str()) {
            bad.push(format!("state {:?} is not one of {STATES:?}", cols[2]));
            continue;
        }
        rows.push(Gate {
            name: cols[0].clone(),
            command: first_backtick(&cols[1]),
            state: cols[2].clone(),
        });
    }
    (rows, bad)
}

fn show(out: &Output) {
    for stream in [&out.stdout[..], &out.stderr[..]] {
        print!("{}", String::from_utf8_lossy(stream));
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| AGENTS.to_string());
    let Ok(md) = fs::read_to_string(&path) else {
        eprintln!("gates: cannot read {path}");
        exit(1);
    };
    let (rows, bad) = gates(&md);
    if rows.is_empty() {
        eprintln!("gates: no gate rows in {path}");
        exit(1);
    }
    let mut failed = bad.len();
    for finding in &bad {
        println!("{:<23}FAIL  {finding}", "malformed row");
    }
    let runnable = rows
        .iter()
        .filter(|r| r.state == "run" && !r.command.contains(THIS_LANE))
        .count();
    for row in &rows {
        print!("{:<23}", row.name);
        let _ = std::io::stdout().flush();
        // The lane row runs this binary, so running it here would recurse. The
        // command is the test, not the row name: renaming the row keeps the skip.
        if row.command.contains(THIS_LANE) {
            println!("skipped (this lane)");
            continue;
        }
        if row.state != "run" {
            println!("skipped ({})", row.state);
            continue;
        }
        if row.command.is_empty() {
            println!("FAIL  command cell has no command");
            failed += 1;
            continue;
        }
        match std::process::Command::new("sh")
            .arg("-c")
            .arg(&row.command)
            .output()
        {
            Ok(out) if out.status.success() => println!("PASS"),
            Ok(out) => {
                println!("FAIL  {}", row.command);
                show(&out);
                failed += 1;
            }
            Err(e) => {
                println!("FAIL  cannot run {}: {e}", row.command);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        eprintln!("gates: {failed} of {runnable} gates failed");
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = r#"## Gates

| Gate | Command | State |
|---|---|---|
| a run | `true` prose | run |
| escaped | `test -z "$(ls \| grep -q x)` | run |
| scaffold | `cargo run --example x` prose | M0 |
| by hand | `cargo run --bin eval` prose | manual |
| no command | prose only | run |
| unescaped | `ls | grep -q x` | run |

## Other

| Gate | Command | State |
|---|---|---|
| outside | `true` | run |
"#;

    #[test]
    fn only_the_gates_table_is_read() {
        let rows = gates(TABLE).0;
        let names: Vec<&str> = rows.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["a run", "escaped", "scaffold", "by hand", "no command"]
        );
    }

    #[test]
    fn an_escaped_pipe_stays_in_the_command() {
        let row = gates(TABLE)
            .0
            .into_iter()
            .find(|g| g.name == "escaped")
            .unwrap();
        assert_eq!(row.command, r#"test -z "$(ls | grep -q x)"#);
    }

    #[test]
    fn a_row_the_lane_cannot_run_is_a_finding() {
        let (rows, bad) = gates(TABLE);
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("need 3"));
        assert!(!rows.iter().any(|g| g.name == "unescaped"));
    }

    #[test]
    fn a_degenerate_row_is_a_finding_not_a_panic() {
        let (rows, bad) = gates("## Gates\n\n|\n| a | `true` | run |\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn an_indented_row_is_still_a_gate() {
        let (rows, bad) = gates("## Gates\n\n  | indented | `true` | run |  \n");
        assert_eq!(rows.len(), 1);
        assert!(bad.is_empty());
    }

    #[test]
    fn the_lane_row_skips_on_its_command_not_its_name() {
        let md = "## Gates\n\n| renamed | `cargo run -p tau-tools --bin gates` | run |\n";
        let (rows, _) = gates(md);
        assert!(rows[0].command.contains(THIS_LANE));
    }

    #[test]
    fn a_command_is_the_first_backtick_span() {
        assert_eq!(
            first_backtick("`cargo fmt` and `cargo clippy`"),
            "cargo fmt"
        );
        assert_eq!(first_backtick("prose `no`"), "no");
        assert_eq!(first_backtick("prose only"), "");
    }

    #[test]
    fn states_survive_the_scan() {
        let rows = gates(TABLE).0;
        let states: Vec<&str> = rows.iter().map(|g| g.state.as_str()).collect();
        assert_eq!(states, ["run", "run", "M0", "manual", "run"]);
    }
}
