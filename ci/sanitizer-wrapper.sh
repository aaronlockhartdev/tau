#!/bin/sh
# rustc refuses to build proc-macro units with a sanitizer active
# (they load into the compiler; rust-lang/rust#160585), and on a
# same-arch runner no RUSTFLAGS scope separates host units (proc-macros,
# build scripts, their deps) from target units. So the sanitizer flag is
# added only for workspace members — via RUSTC_WORKSPACE_WRAPPER, which
# cargo invokes for exactly those units — leaving every external
# dependency, including all proc-macros, uninstrumented. ASan still
# guards all heap traffic from the workspace code; SANITIZER selects the
# kind (address | thread). RUSTC wrapper protocol: $1 is the rustc path.
rustc_bin="$1"; shift
if [ "$SANITIZER" = "thread" ]; then
  # TSan demands consistent instrumentation across the whole link and
  # hard-errors on the sanctioned mixed setup (workspace instrumented,
  # deps not) — the compiler's own escape hatch applies the exception.
  exec "$rustc_bin" -Zsanitizer=thread -Cunsafe-allow-abi-mismatch=sanitizer "$@"
fi
exec "$rustc_bin" -Zsanitizer="$SANITIZER" "$@"
