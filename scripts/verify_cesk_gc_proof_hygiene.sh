#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$SCRIPT_DIR/.." && pwd)"
FORMAL_HARNESS="$REPO/scripts/verify_cesk_gc_formal.sh"

failures=0

word_pattern() {
  local words="$1"
  printf '(^|[^[:alnum:]_])(%s)([^[:alnum:]_]|$)' "$words"
}

check_forbidden() {
  local label="$1" pattern="$2"
  shift 2
  local files=("$@")
  local file

  for file in "${files[@]}"; do
    if LC_ALL=C grep -nE "$pattern" "$file"; then
      echo "proof hygiene: forbidden $label token in ${file#$REPO/}" >&2
      failures=1
    fi
  done
}

require_harness_entry() {
  local runner="$1" file="$2"
  local rel="${file#$REPO/}"

  if ! grep -qF "$runner \"$rel\"" "$FORMAL_HARNESS"; then
    echo "proof hygiene: $rel is not enumerated by $FORMAL_HARNESS" >&2
    failures=1
  fi
}

mapfile -t lean_files < <(find "$REPO/formal/lean/gc" -maxdepth 1 -type f -name '*.lean' | sort)
mapfile -t rocq_files < <(find "$REPO/formal/rocq/gc" -maxdepth 1 -type f -name '*.v' | sort)
mapfile -t workpool_rocq_files < <(find "$REPO/formal/rocq/work_pool_stability/theories" -maxdepth 1 -type f -name '*.v' | sort)

if [[ "${#rocq_files[@]}" -eq 0 ]]; then
  echo "proof hygiene: expected Rocq CESK GC proof files" >&2
  exit 1
fi

if [[ "${#lean_files[@]}" -eq 0 ]]; then
  echo "proof hygiene: expected Lean GC proof mirror files" >&2
  exit 1
fi

if [[ "${#workpool_rocq_files[@]}" -eq 0 ]]; then
  echo "proof hygiene: expected Rocq WorkPool stability proof files" >&2
  exit 1
fi

if [[ "${#lean_files[@]}" -ne 0 ]]; then
  check_forbidden "supplemental Lean proof shortcut" \
    "$(word_pattern 'sorry|admit|axiom|constant|opaque|unsafe')" \
    "${lean_files[@]}"
fi

check_forbidden "Rocq proof shortcut" \
  "$(word_pattern 'Admitted|admit|Axiom|Axioms|Parameter|Parameters|Conjecture|Conjectures|Abort')" \
  "${rocq_files[@]}" "${workpool_rocq_files[@]}"

for file in "${rocq_files[@]}"; do
  require_harness_entry "run_rocq" "$file"
done

for file in "${workpool_rocq_files[@]}"; do
  require_harness_entry "run_workpool_rocq" "$file"
done

if [[ "$failures" -ne 0 ]]; then
  exit 1
fi

echo "CESK GC proof-hygiene checks passed (${#rocq_files[@]} GC Rocq mandatory, ${#workpool_rocq_files[@]} WorkPool Rocq mandatory, ${#lean_files[@]} Lean mandatory)"
