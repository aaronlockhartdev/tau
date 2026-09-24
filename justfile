# The single build/acceptance entry point (roadmap F, ticket #27): all
# targets delegate to the native tools (cargo, npm/vite, tauri).

default:
    @just build

build:
    #!/bin/sh
    set -eu
    cd "{{justfile_directory()}}"
    cargo build --workspace --release
    (cd app && npm ci && npm run build)
    if [ "$(uname)" = "Darwin" ]; then
      (cd app && npx tauri build --bundles app)
    else
      (cd app && npx tauri build --bundles appimage)
    fi

test:
    #!/bin/sh
    set -eu
    cd "{{justfile_directory()}}"
    cargo nextest run --workspace
    (cd app && npm run test)

# Spec §1 in-scope legs; `just acceptance <leg…>` filters (default: all).
# Live legs (b/c/d) run only when TAU_LIVE=1 — a skipped live leg is not a
# failure, a red leg is.
acceptance *legs = 'a b c d e f':
    #!/bin/sh
    set -u
    cd "{{justfile_directory()}}"

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

    run_driver() {
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

    launch_smoke() {
      # $1 = the launch command. Survival is judged at the FINAL check only:
      # a startup panic must fail the leg, not pass on an early "alive" sample.
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
        report a PASS "the built app launched and stayed up 15 s (smoke)"
      else
        report a FAIL "the app exited before 15 s of launch"
      fi
    }

    echo "tau v0 acceptance — $(date -u '+%Y-%m-%d %H:%M UTC')"
    echo "endpoint: $TAU_ENDPOINT  model: $TAU_MODEL  live: $TAU_LIVE"
    echo ""

    for leg in {{legs}}; do
      case "$leg" in
        a)
          if just build > /tmp/tau-acceptance-build.log 2>&1; then
            if [ "$(uname)" = "Darwin" ]; then
              bin="target/release/bundle/macos/Tau.app/Contents/MacOS/tau-app"
              if [ ! -x "$bin" ]; then
                report a FAIL "the build reported success but $bin is missing"
              else
                launch_smoke "$bin"
              fi
            else
              # WebKitGTK wants an X display: headless Linux runs under xvfb (spec §13).
              bin="target/release/tau-app"
              if [ ! -x "$bin" ]; then
                report a FAIL "the build reported success but $bin is missing"
              else
                launch_smoke "xvfb-run -a $bin"
              fi
            fi
          else
            report a FAIL "just build failed (see /tmp/tau-acceptance-build.log)"
          fi
          ;;
        b|c|d)
          run_driver "$leg"
          ;;
        e)
          out=$(./target/release/tau-acceptance e 2>&1); status=$?
          if [ $status -eq 0 ]; then
            report e PASS "$(echo "$out" | tail -1)"
          else
            report e FAIL "$(echo "$out" | tail -1)"
          fi
          ;;
        f)
          if command -v node >/dev/null 2>&1; then
            out=$(node app/tests/e2e/run-e2e.mjs 2>&1); status=$?
            if [ $status -eq 0 ]; then
              n=$(echo "$out" | grep -c '^PASS')
              report f PASS "$n/$(echo "$out" | grep -cE '^(PASS|FAIL)') E2E checks (real app on the 10k-entry fixture, 2 canned 25 ms streams)"
            else
              report f FAIL "$(echo "$out" | grep -m1 '^FAIL' || echo 'the real-app E2E failed')"
            fi
          else
            report f SKIP "node is not available"
          fi
          ;;
        *)
          report "$leg" FAIL "unknown leg"
          ;;
      esac
    done

    echo ""
    echo "summary: $pass passed, $fail failed, $skip skipped"
    [ "$fail" -eq 0 ]
