#!/usr/bin/env bash
# Retired compatibility wrapper for the old one-binary C1.b benchmark.
#
# C1.c deleted the young-minor on/off environment switch. A generational GC A/B
# must compare two git revisions, not one binary under two env settings. Keep
# this filename so old notes and shells fail into the current benchmark shape
# instead of silently running a stale methodology.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

usage() {
  cat >&2 <<'USAGE'
scripts/c1b_fanout0_bench.sh is retired.

Use:
  scripts/c_ab_bench.sh <A_REF> <B_REF> [reps]

This wrapper accepts the same arguments and forwards them to c_ab_bench.sh.
USAGE
}

if [[ "${1:-}" == "--help" || "$#" -lt 2 ]]; then
  usage
  exit 2
fi

exec "$SCRIPT_DIR/c_ab_bench.sh" "$@"
