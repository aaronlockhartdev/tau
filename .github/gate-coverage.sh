#!/usr/bin/env bash
# Gate: `ci covers gates` (R-workspace-12).
# Every gate row in state `run` must name a command the CI workflow runs, so
# editing a gate row without editing the workflow is a finding, not drift.
# A command cell is the first backticked segment of a `run` row; a row with no
# backticks is prose, and later backticked segments are prose too.
# A `{a,b,c}` placeholder matches on the text before it.
set -uo pipefail
table=AGENTS.md
workflow=.github/workflows/gates.yml
[[ -f "$table" && -f "$workflow" ]] || { echo "ci covers gates: need $table and $workflow"; exit 1; }
awk -v wf="$workflow" '
  BEGIN { FS = "|"; while ((getline line < wf) > 0) all = all "\n" line; close(wf) }
  /^\|/ {
    state = $4; gsub(/[[:space:]]/, "", state);
    if (state != "run") next;
    cell = $3;
    # The first backticked segment is the command; later ones are prose.
    if (!match(cell, /`[^`]+`/)) next;
    cmd = substr(cell, RSTART + 1, RLENGTH - 2);
    gsub(/ *&& *| *; */, "\n", cmd);
    n = split(cmd, parts, "\n");
    for (i = 1; i <= n; i++) {
      p = parts[i];
      sub(/\{.*/, "", p);
      gsub(/[[:space:]]+$/, "", p);
      if (p == "") continue;
      if (index(all, p) == 0) { print "no workflow step runs: " p; bad = 1; }
    }
  }
  END { exit bad }
' "$table"
