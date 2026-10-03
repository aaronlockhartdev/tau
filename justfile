# The single build/acceptance entry point (roadmap F, ticket #27): all
# targets delegate to the native tools (cargo, npm/vite, tauri).

default:
    @just build

# The development loop: a debug build (tauri-pilot's socket is debug-only,
# The development loop: a debug build with the e2e feature (the pilot +
# WebdriverIO plugins are feature-gated, sweep finding X1) plus the Svelte
# dev server and hot reload.
dev:
    #!/bin/sh
    set -eu
    cd "{{justfile_directory()}}/app"
    npx tauri dev --features e2e

build:
    #!/bin/sh
    set -eu
    cd "{{justfile_directory()}}"
    cargo build --workspace --release
    (cd app && npm ci && npm run build)
    if [ "$(uname)" = "Darwin" ]; then
      (cd app && npx tauri build --bundles app,dmg)
    else
      (cd app && npx tauri build --bundles appimage)
    fi

test:
    #!/bin/sh
    set -eu
    cd "{{justfile_directory()}}"
    cargo nextest run --workspace
    (cd app && npm run test)

# Spec §1 in-scope suites; `just acceptance <suite…>` filters (default: all).
# The accept-* suites run against the deterministic mock LLM only (ADR-0010):
# red = a code problem, never a network/model problem; real-model runs live
# in the eval rig's live leg, not here.
acceptance *suites = 'launch accept-tools accept-subagent accept-om core e2e':
    #!/bin/sh
    set -u
    cd "{{justfile_directory()}}"

    MOCK_PORT="${MOCK_PORT:-8123}"
    TAU_ENDPOINT="http://127.0.0.1:$MOCK_PORT/v1"
    mock_pid=""

    pass=0
    fail=0
    skip=0
    start_ts=$(date +%s)

    # Color only on a terminal; piped/CI output stays plain.
    if [ -t 1 ]; then
      B=$(printf '\033[1m'); G=$(printf '\033[32m'); R=$(printf '\033[31m')
      Y=$(printf '\033[33m'); D=$(printf '\033[2m'); N=$(printf '\033[0m')
    else
      B=""; G=""; R=""; Y=""; D=""; N=""
    fi
    report() {
      # $1 = suite, $2 = status (PASS|FAIL|SKIP), $3 = detail
      case "$2" in
        PASS) pass=$((pass + 1)); c=$G ;;
        FAIL) fail=$((fail + 1)); c=$R ;;
        SKIP) skip=$((skip + 1)); c=$Y ;;
      esac
      printf '%s%-14s%s  %s%s%s  %s\n' "$B" "$1" "$N" "$c" "$2" "$N" "$3"
    }

    detail() {
      # $1 = label, $2 = output. A failure's own words must be visible
      # without hunting for a log file.
      if [ -n "$2" ]; then
        printf '%s-- %s output --%s\n' "$D" "$1" "$N"
        printf '%s%s%s\n' "$D" "$2" "$N"
      fi
      return 0
    }

    start_mock() {
      # The deterministic mock LLM (phase 1 §4): `just build` already builds
      # it (workspace member); a standalone run gets a targeted build.
      if [ ! -x target/release/tau-mock-llm ]; then
        cargo build --release -p tau-mock-llm > /tmp/tau-mock-build.log 2>&1 || return 1
      fi
      : > /tmp/tau-mock-llm.log
      ./target/release/tau-mock-llm --port "$MOCK_PORT" --scenarios fixtures/e2e-mocks > /tmp/tau-mock-llm.log 2>&1 &
      mock_pid=$!
      i=0
      while [ $i -lt 100 ]; do
        grep -q "listening" /tmp/tau-mock-llm.log 2>/dev/null && return 0
        kill -0 "$mock_pid" 2>/dev/null || return 1
        i=$((i + 1))
        sleep 0.1
      done
      return 1
    }

    run_driver() {
      # $1 = suite, $2... = driver args. The suites are mock-only (ADR-0010);
      # TAU_ENDPOINT is the mock's address.
      suite="$1"; shift
      # One mock for the whole run: a second start would fail the port
      # rebind and every later suite would report a mock failure.
      if [ -z "$mock_pid" ]; then
        start_mock || {
          report "$suite" FAIL "the mock LLM failed to start"
          detail "mock-llm" "$(cat /tmp/tau-mock-llm.log 2>/dev/null)"
          return 1
        }
      fi
      out=$(TAU_ENDPOINT="$TAU_ENDPOINT" \
        ./target/release/tau-test "$suite" "$@" 2>&1)
      status=$?
      if [ $status -eq 0 ]; then
        report "$suite" PASS "$(echo "$out" | tail -1)"
      else
        report "$suite" FAIL "$(echo "$out" | tail -1)"
        detail "$suite" "$out"
      fi
      return $status
    }

    launch_smoke() {
      # $1 = the launch command. Survival is judged at the FINAL check only:
      # a startup panic must fail the suite, not pass on an early "alive" sample.
      $1 >/dev/null 2>&1 &
      pid=$!
      up=1
      i=0
      while [ $i -lt 15 ]; do
        kill -0 "$pid" 2>/dev/null || up=0
        i=$((i + 1))
        sleep 1
      done
      kill "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      if [ $up -eq 1 ]; then
        report launch PASS "the built app launched and stayed up 15 s (smoke)"
      else
        report launch FAIL "the app exited before 15 s of launch"
      fi
    }

    printf '%s%s%s\n' "$B" "tau v0 acceptance" "$N"
    echo "  $(date -u '+%Y-%m-%d %H:%M UTC')  endpoint: mock:127.0.0.1:$MOCK_PORT  model: mock-model"
    echo ""

    for suite in {{suites}}; do
      case "$suite" in
        launch)
          if just build > /tmp/tau-test-build.log 2>&1; then
            if [ "$(uname)" = "Darwin" ]; then
              bin="target/release/bundle/macos/Tau.app/Contents/MacOS/tau-app"
              if [ ! -x "$bin" ]; then
                report launch FAIL "the build reported success but $bin is missing"
              else
                launch_smoke "$bin"
              fi
            else
              # WebKitGTK wants an X display: headless Linux runs under xvfb (spec §13).
              bin="target/release/tau-app"
              if [ ! -x "$bin" ]; then
                report launch FAIL "the build reported success but $bin is missing"
              else
                launch_smoke "xvfb-run -a $bin"
              fi
            fi
          else
            report launch FAIL "just build failed (see /tmp/tau-test-build.log)"
          fi
          ;;
        accept-tools|accept-subagent|accept-om)
          run_driver "$suite"
          ;;
        core)
          out=$(./target/release/tau-test core 2>&1); status=$?
          if [ $status -eq 0 ]; then
            report core PASS "$(echo "$out" | tail -1)"
          else
            report core FAIL "$(echo "$out" | tail -1)"
            detail core "$out"
          fi
          ;;
        e2e)
          # The e2e leg owns its own mock: stop the shared acceptance mock so
          # the leg's mock can bind $MOCK_PORT fresh. A shared mock has its
          # scenario turn counters consumed by the earlier suites, and the
          # leg's health poll cannot tell a fresh mock from this stranger —
          # scripted specs then answer from polluted state.
          if [ -n "$mock_pid" ]; then
            kill "$mock_pid" 2>/dev/null
            wait "$mock_pid" 2>/dev/null
            mock_pid=""
            i=0
            while [ $i -lt 50 ] && nc -z 127.0.0.1 "$MOCK_PORT" 2>/dev/null; do
              i=$((i + 1))
              sleep 0.1
            done
          fi
          if command -v node >/dev/null 2>&1; then
            # Self-sufficient on a clean checkout (sweep finding X4): the
            # E2E driver needs the debug binary WITH the e2e feature (the
            # embedded WebDriver plugin). The build is a no-op when that
            # exact build is current — but it must not be skipped on mere
            # existence: a feature-less debug build satisfies the test yet
            # lacks the plugin.
            if ! cargo build -p tau-app --features e2e; then
              report e2e FAIL "debug binary build failed"
              continue
            fi
            # The WebKitGTK webview wants an X display (spec §13): headless
            # Linux runs under xvfb, as the launch smoke already does.
            # ${DISPLAY:-}: the script runs under set -u and headless CI has no DISPLAY.
            if [ "$(uname)" = "Linux" ] && [ -z "${DISPLAY:-}" ]; then
              out=$(cd app && xvfb-run -a npm run test:frontend 2>&1)
            else
              out=$(cd app && npm run test:frontend 2>&1)
            fi
            status=$?
            if [ $status -eq 0 ]; then
              n=$(echo "$out" | grep -c 'PASS  ')
              report e2e PASS "$n E2E checks passed (mode ${TAU_E2E_MODE:-all})"
            else
              report e2e FAIL "$(echo "$out" | grep -m1 'FAIL  ' || echo 'the real-app E2E failed')"
              detail e2e "$out"
            fi
          else
            report e2e SKIP "node is not available"
          fi
          ;;
        *)
          report "$suite" FAIL "unknown suite"
          ;;
      esac
    done
    # Reap the killed mock so the shell does not print its job notification.
    if [ -n "$mock_pid" ]; then
      kill "$mock_pid" 2>/dev/null
      wait "$mock_pid" 2>/dev/null
    fi

    total=$(( $(date +%s) - start_ts ))
    echo ""
    printf '%ssummary:%s %s%d passed%s, %d failed, %d skipped  %s(%dm %02ds)%s\n' \
      "$B" "$N" "$G" "$pass" "$N" "$fail" "$skip" "$D" "$((total / 60))" "$((total % 60))" "$N"
    [ "$fail" -eq 0 ]
