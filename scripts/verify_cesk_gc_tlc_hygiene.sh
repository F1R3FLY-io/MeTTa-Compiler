#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$SCRIPT_DIR/.." && pwd)"
FORMAL_HARNESS="$REPO/scripts/verify_cesk_gc_formal.sh"
TLA_DIR="$REPO/tla"

failures=0

fail() {
  echo "TLC hygiene: $*" >&2
  failures=1
}

declare -A run_labels=()
declare -A run_cfgs=()
declare -A run_modules=()
declare -A skipped_cfgs=()
declare -A skipped_modules=()
declare -A covered_modules=()

skip_cfg() {
  local cfg="$1" reason="$2"
  skipped_cfgs["$cfg"]="$reason"
}

skip_module() {
  local module="$1" reason="$2"
  skipped_modules["$module"]="$reason"
}

mark_extended_modules() {
  local module="$1" source="$2" line dep

  [[ -f "$TLA_DIR/$module" ]] || return 0

  while IFS= read -r line; do
    line="${line#EXTENDS }"
    line="${line//,/ }"
    for dep in $line; do
      case "$dep" in
        Naturals|Integers|Sequences|FiniteSets|TLC|TLCExt|Toolbox|Bags|Reals)
          ;;
        *)
          if [[ -f "$TLA_DIR/$dep.tla" ]]; then
            covered_modules["$dep.tla"]="$source"
          fi
          ;;
      esac
    done
  done < <(sed -n 's/^[[:space:]]*EXTENDS[[:space:]]\+/EXTENDS /p' "$TLA_DIR/$module")
}

skip_cfg "MC_GenYoungMark_positive.cfg" \
  "full historical young-only-marker positive discriminator; harness runs MC_GenYoungMark_positive_small.cfg"
skip_cfg "MC_GenYoungMark_negative.cfg" \
  "full historical young-only-marker negative discriminator; harness runs MC_GenYoungMark_negative_small.cfg"
skip_cfg "MC_StoreCentricGC_Generational.cfg" \
  "larger C1 generational full-mark config; harness runs MC_StoreCentricGC_Generational_small.cfg"

skip_cfg "MC_StoreCentricGC.cfg" \
  "older non-generational store-centric mark-sweep config outside the active CESK generational gate"
skip_cfg "MC_StoreCentricGC_small.cfg" \
  "older non-generational store-centric mark-sweep smoke config outside the active CESK generational gate"
skip_cfg "MC_StoreCentricGC_liveness.cfg" \
  "older non-generational store-centric mark-sweep liveness config outside the active CESK generational gate"
skip_module "MC_StoreCentricGC.tla" \
  "older non-generational store-centric mark-sweep wrapper outside the active CESK generational gate"

for cfg in \
  SlabGC.cfg \
  SlabGC_datarace.cfg \
  SlabGC_deadlock.cfg \
  SlabGC_doublefree.cfg \
  SlabGC_Pages.cfg \
  SlabGC_Quiescent.cfg \
  SlabGC_Quiescent_deadlock.cfg \
  SlabGC_Quiescent_small.cfg \
  SlabGC_Reactive.cfg \
  SlabGC_Reactive_deadlock.cfg \
  SlabGC_Reactive_deadlock_small.cfg \
  SlabGC_Reactive_medium.cfg \
  SlabGC_Reactive_small.cfg; do
  skip_cfg "$cfg" "legacy slab mark-sweep model outside the CESK generational proof boundary"
done

for module in \
  MC_SlabGC.tla \
  MC_SlabGC_Quiescent.tla \
  MC_SlabGC_Reactive.tla; do
  skip_module "$module" "legacy slab mark-sweep wrapper outside the CESK generational proof boundary"
done

skip_module "SlabGC_Pages.tla" \
  "legacy slab page-shape support module outside the CESK generational proof boundary"

mapfile -t tlc_runs < <(
  awk '
    /^[[:space:]]*run_tlc[[:space:]]+"/ {
      line = $0
      while (line ~ /\\[[:space:]]*$/ && getline next_line > 0) {
        sub(/\\[[:space:]]*$/, "", line)
        line = line " " next_line
      }
      gsub(/[[:space:]]+/, " ", line)
      print line
    }
  ' "$FORMAL_HARNESS"
)

if [[ "${#tlc_runs[@]}" -eq 0 ]]; then
  fail "no run_tlc entries found in ${FORMAL_HARNESS#$REPO/}"
fi

for line in "${tlc_runs[@]}"; do
  if [[ ! "$line" =~ ^run_tlc[[:space:]]+\"([^\"]+)\"[[:space:]]+\"([^\"]+)\"[[:space:]]+\"([^\"]+)\"[[:space:]]+(pass|fail)[[:space:]]+\"([^\"]*)\"[[:space:]]*$ ]]; then
    fail "could not parse run_tlc entry: $line"
    continue
  fi

  label="${BASH_REMATCH[1]}"
  module="${BASH_REMATCH[2]}"
  cfg="${BASH_REMATCH[3]}"
  expect="${BASH_REMATCH[4]}"
  pattern="${BASH_REMATCH[5]}"

  if [[ -n "${run_labels[$label]+seen}" ]]; then
    fail "duplicate run_tlc label '$label'"
  fi
  run_labels["$label"]=1

  if [[ -n "${run_cfgs[$cfg]+seen}" ]]; then
    fail "duplicate TLC config in harness: $cfg"
  fi
  run_cfgs["$cfg"]=1
  run_modules["$module"]=1

  [[ -f "$TLA_DIR/$module" ]] || fail "missing TLC module $module"
  [[ -f "$TLA_DIR/$cfg" ]] || fail "missing TLC config $cfg"

  [[ "$module" == *.tla ]] || fail "TLC module is not a .tla file: $module"
  [[ "$cfg" == *.cfg ]] || fail "TLC config is not a .cfg file: $cfg"

  case "$expect" in
    pass)
      [[ -z "$pattern" ]] || fail "positive TLC run '$label' has unexpected discriminator '$pattern'"
      ;;
    fail)
      [[ -n "$pattern" ]] || fail "negative TLC run '$label' lacks a discriminator pattern"
      ;;
    *)
      fail "invalid TLC expectation '$expect' for '$label'"
      ;;
  esac
done

for cfg in "${!skipped_cfgs[@]}"; do
  [[ -f "$TLA_DIR/$cfg" ]] || fail "classified skipped config no longer exists: $cfg"
  if [[ -n "${run_cfgs[$cfg]+seen}" ]]; then
    fail "config $cfg is both run and classified as skipped"
  fi
done

for module in "${!skipped_modules[@]}"; do
  [[ -f "$TLA_DIR/$module" ]] || fail "classified skipped wrapper no longer exists: $module"
  if [[ -n "${run_modules[$module]+seen}" ]]; then
    fail "wrapper $module is both run and classified as skipped"
  fi
done

mapfile -t cfg_files < <(git -C "$REPO" ls-files 'tla/*.cfg' | sed 's#^tla/##' | sort)
for cfg in "${cfg_files[@]}"; do
  if [[ -z "${run_cfgs[$cfg]+seen}" && -z "${skipped_cfgs[$cfg]+seen}" ]]; then
    fail "TLA config $cfg is neither run by the formal harness nor explicitly classified"
  fi
done

mapfile -t wrapper_modules < <(git -C "$REPO" ls-files 'tla/MC_*.tla' | sed 's#^tla/##' | sort)
for module in "${wrapper_modules[@]}"; do
  if [[ -z "${run_modules[$module]+seen}" && -z "${skipped_modules[$module]+seen}" ]]; then
    fail "TLC wrapper $module is neither run by the formal harness nor explicitly classified"
  fi
done

for module in "${!run_modules[@]}"; do
  mark_extended_modules "$module" "run module $module"
done

for module in "${!skipped_modules[@]}"; do
  mark_extended_modules "$module" "classified module $module"
done

mapfile -t tla_modules < <(git -C "$REPO" ls-files 'tla/*.tla' | sed 's#^tla/##' | sort)
for module in "${tla_modules[@]}"; do
  if [[ -z "${run_modules[$module]+seen}" \
     && -z "${skipped_modules[$module]+seen}" \
     && -z "${covered_modules[$module]+seen}" ]]; then
    fail "TLA module $module is neither run, imported by a run/classified wrapper, nor explicitly classified"
  fi
done

if [[ "$failures" -ne 0 ]]; then
  exit 1
fi

echo "CESK GC TLC-hygiene checks passed (${#run_cfgs[@]} TLC configs run, ${#covered_modules[@]} modules imported, ${#skipped_cfgs[@]} configs classified)"
