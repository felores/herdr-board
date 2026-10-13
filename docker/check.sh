#!/usr/bin/env bash
# Focused checkpoint runner inside the herdr-board Docker sandbox.
set -euo pipefail
cd /repo

export CARGO_NET_OFFLINE=true
export HB_SELFCHECK_NETWORK=off
bash /repo/docker/selfcheck.sh

mode="${1:-}"
[ -n "$mode" ] || { echo "sandbox check: missing mode" >&2; exit 2; }
shift

case "$mode" in
  rust)
    cargo test --workspace --all-features "$@"
    ;;
  python)
    if [ "$#" -eq 0 ]; then
      python3 -m unittest discover -s scripts/tests -p 'test_*.py'
    elif [ "$#" -eq 1 ]; then
      python3 -m unittest discover -s scripts/tests -p "$1"
    else
      echo "sandbox check: python accepts at most one filename pattern" >&2
      exit 2
    fi
    ;;
  harness)
    [ "$#" -eq 0 ] || { echo "sandbox check: harness accepts no extra arguments" >&2; exit 2; }
    bash /repo/e2e/test-harness.sh
    ;;
  e2e)
    export E2E_FORCE_BUILD=1
    export HERDR_BIN_PATH=/usr/local/bin/herdr
    export BOARD_BIN="$CARGO_TARGET_DIR/release/board"
    bash /repo/e2e/run-all.sh "$@"
    ;;
  fmt)
    [ "$#" -eq 0 ] || { echo "sandbox check: fmt accepts no extra arguments" >&2; exit 2; }
    cargo fmt --all --check
    ;;
  clippy)
    [ "$#" -eq 0 ] || { echo "sandbox check: clippy accepts no extra arguments" >&2; exit 2; }
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    ;;
  *)
    echo "sandbox check: unknown mode '$mode'" >&2
    exit 2
    ;;
esac
