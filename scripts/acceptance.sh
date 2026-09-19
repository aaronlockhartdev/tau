#!/bin/sh
# End-to-end acceptance (ticket #27): the spec §1 in-scope list, proven
# from the repo. Legs:
#   a  fresh install: ./build, then the built app launches and stays up (macOS)
#   b  multi-turn with the four core tools + a golden-file edit (live)
#   c  a model-spawned sub-agent works a task and wakes the parent (live)
#   d  OM compaction on a synthesized long session (live observe/reflect)
#   e  branching + manual archive round-trip (offline)
#   f  the #11 performance bar: 10k-entry demo + 2 live 25 ms streams
#
# Live legs (b, c, d) run only when TAU_LIVE=1 (defaults: the dev endpoint
# from dev/config.toml); without it they print SKIP — a skipped live leg is
# not a failure, a red leg is. Each live call is capped at 300 output
# tokens; the whole live budget is ~8 small generations.

set -u

cd "$(dirname "$0")/.."

TAU_LIVE="${TAU_LIVE:-0}"
TAU_ENDPOINT="${TAU_ENDPOINT:-https://llms.aaronlockhart.dev/v1}"
TAU_MODEL="${TAU_MODEL:-qwen3.8-27b}"

pass=0
fail=0
skip=0

report() {
  # $1 = leg, $2 = status (PASS|FAIL|SKIP), $3 = detail
  case "$2" in
    PASS) pass=$((pass + 1)) ;;
    FAIL) fail=$((fail + 1)) ;;
    SKIP) skip=$((skip + 1)) ;;
  esac
  printf '%-28s %s  %s\n' "leg $1" "$2" "$3"
}

run_live() {
  # $1 = leg, $2... = driver args
  leg="$1"; shift
  if [ "$TAU_LIVE" != "1" ]; then
    report "$leg" SKIP "TAU_LIVE unset (set TAU_LIVE=1 with TAU_ENDPOINT/TAU_MODEL)"
    return 0
  fi
  out=$(TAU_ENDPOINT="$TAU_ENDPOINT" TAU_MODEL="$TAU_MODEL" \
    ./target/release/tau-acceptance "$leg" "$@" 2>&1)
  status=$?
  if [ $status -eq 0 ]; then
    report "$leg" PASS "$(echo "$out" | tail -1)"
  else
    report "$leg" FAIL "$(echo "$out" | tail -1)"
  fi
  return $status
}

echo "tau v0 acceptance — $(date -u '+%Y-%m-%d %H:%M UTC')"
echo "endpoint: $TAU_ENDPOINT  model: $TAU_MODEL  live: $TAU_LIVE"
echo ""

# -- a: fresh install (the build) + the app launches ----------------------
if [ "$(uname)" = "Darwin" ]; then
  if ./build > /tmp/tau-acceptance-build.log 2>&1; then
    app=target/release/bundle/macos/Tau.app
    bin="$app/Contents/MacOS/tau-app"
    if [ ! -x "$bin" ]; then
      report a FAIL "the build reported success but $bin is missing"
    else
      "$bin" >/dev/null 2>&1 &
      pid=$!
      up=1
      i=0
      while [ $i -lt 15 ]; do
        # survival is judged at the FINAL check only: a startup panic must
        # fail the leg, not pass it on an early "alive" sample
        kill -0 "$pid" 2>/dev/null || up=0
        i=$((i + 1))
        sleep 1
      done
      kill "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      if [ $up -eq 1 ]; then
        report a PASS "the built Tau.app launched and stayed up 15 s (smoke)"
      else
        report a FAIL "the app exited before 15 s of launch"
      fi
    fi
  else
    report a FAIL "./build failed (see /tmp/tau-acceptance-build.log)"
  fi
else
  # Linux: the app bundle needs webkit system libraries; the build (core +
  # frontend) is checked below with the other legs. This pins how far Linux
  # is exercised (map fog item): everything except the GUI smoke.
  report a SKIP "the app-launch smoke is macOS-only (no webkit bundle on Linux)"
fi

# -- b: multi-turn, all four core tools (live) ----------------------------
run_live b

# -- c: sub-agent + task through the live system (live) -------------------
run_live c

# -- d: OM compaction on a long session (live) ----------------------------
run_live d

# -- e: branching + manual archive (offline) ------------------------------
out=$(./target/release/tau-acceptance e 2>&1); status=$?
if [ $status -eq 0 ]; then
  report e PASS "$(echo "$out" | tail -1)"
else
  report e FAIL "$(echo "$out" | tail -1)"
fi

# -- f: the #11 performance bar (the committed demo verification) ---------
if command -v node >/dev/null 2>&1; then
  if [ ! -d app/node_modules ]; then
    (cd app && npm ci >/dev/null 2>&1)
  fi
  out=$(node app/scripts/verify-demo.mjs 2>&1); status=$?
  if [ $status -eq 0 ]; then
    n=$(echo "$out" | grep -c '^PASS')
    report f PASS "$n/$(echo "$out" | grep -cE '^(PASS|FAIL)') demo checks (10k entries, 2 live 25 ms streams)"
  else
    report f FAIL "$(echo "$out" | grep -m1 '^FAIL' || echo 'the demo verification failed')"
  fi
else
  report f SKIP "node is not available"
fi

echo ""
echo "summary: $pass passed, $fail failed, $skip skipped"
[ $fail -eq 0 ]
