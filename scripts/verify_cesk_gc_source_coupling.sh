#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd -- "$SCRIPT_DIR/.." && pwd)"

fail() {
  echo "CESK GC source-coupling check failed: $*" >&2
  exit 1
}

line_no() {
  local file="$1" needle="$2" line
  line="$(rg -n -F -m 1 -- "$needle" "$REPO/$file" | cut -d: -f1 || true)"
  [[ -n "$line" ]] || fail "missing '$needle' in $file"
  printf '%s\n' "$line"
}

line_no_after() {
  local file="$1" marker="$2" needle="$3" marker_line line
  marker_line="$(line_no "$file" "$marker")"
  line="$(rg -n -F -- "$needle" "$REPO/$file" | awk -F: -v marker="$marker_line" '$1 > marker { print $1; exit }')"
  [[ -n "$line" ]] || fail "missing '$needle' after '$marker' in $file"
  printf '%s\n' "$line"
}

count_no() {
  local file="$1" needle="$2"
  (rg -n -F -- "$needle" "$REPO/$file" || true) | wc -l | tr -d ' '
}

count_between() {
  local file="$1" start="$2" end="$3" needle="$4" start_line end_line
  start_line="$(line_no "$file" "$start")"
  end_line="$(line_no_after "$file" "$start" "$end")"
  awk -v start="$start_line" -v end="$end_line" -v needle="$needle" \
    'NR > start && NR < end && index($0, needle) { count++ } END { print count + 0 }' \
    "$REPO/$file"
}

assert_before() {
  local file="$1" before="$2" after="$3" before_line after_line
  before_line="$(line_no "$file" "$before")"
  after_line="$(line_no "$file" "$after")"
  if (( before_line >= after_line )); then
    fail "expected '$before' before '$after' in $file (lines $before_line >= $after_line)"
  fi
}

assert_after_before() {
  local file="$1" marker="$2" before="$3" after="$4" before_line after_line
  before_line="$(line_no_after "$file" "$marker" "$before")"
  after_line="$(line_no_after "$file" "$marker" "$after")"
  if (( before_line >= after_line )); then
    fail "expected '$before' after '$marker' and before '$after' in $file (lines $before_line >= $after_line)"
  fi
}

assert_count() {
  local file="$1" needle="$2" expected="$3" actual
  actual="$(count_no "$file" "$needle")"
  if [[ "$actual" != "$expected" ]]; then
    fail "expected $expected occurrence(s) of '$needle' in $file, found $actual"
  fi
}

assert_zero_between() {
  local file="$1" start="$2" end="$3" needle="$4" actual
  actual="$(count_between "$file" "$start" "$end" "$needle")"
  if [[ "$actual" != "0" ]]; then
    fail "expected zero occurrence(s) of '$needle' between '$start' and '$end' in $file, found $actual"
  fi
}

assert_zero() {
  local file="$1" needle="$2" actual
  actual="$(count_no "$file" "$needle")"
  if [[ "$actual" != "0" ]]; then
    fail "expected zero occurrence(s) of '$needle' in $file, found $actual"
  fi
}

# A5 structural-root architecture: the dynamic root registry and raw frame-chain
# discovery path must remain slab-only. The index collector reads roots from the
# reified CESK machine plus narrow driver transport channels.
assert_before \
  "src/backend/models/mod.rs" \
  "#[cfg(not(feature = \"index-gc\"))]" \
  "pub use gc_allocator::{collect_all_roots, register_root_provider, trigger_gc_cycle, RootProvider};"
assert_before \
  "src/backend/eval/mod.rs" \
  "#[cfg(not(feature = \"index-gc\"))]" \
  "pub(crate) mod frame_chain;"
assert_before \
  "src/backend/models/gc_allocator.rs" \
  "#[cfg(not(feature = \"index-gc\"))]" \
  "pub trait RootProvider: Send + Sync {"

# E1 rendezvous safety: the dedicated driver must wait on the V4 witness, publish
# witness_ok, build the root union, run the oracle, and only then enter the
# rendezvous collection gate.
assert_zero "src/backend/eval/cesk/gc_driver.rs" "ga::requestor_wait_for_parked_count"
assert_before "src/backend/eval/cesk/gc_driver.rs" "let snap = ga::snapshot_witness(cur_gen);" "ga::requestor_wait_for_all_reified_parked(&snap, cur_gen);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::requestor_wait_for_all_reified_parked(&snap, cur_gen);" "ga::set_current_witness_ok(true);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::set_current_witness_ok(true);" "ga::drain_worker_root_buffer(&mut roots);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::drain_worker_root_buffer(&mut roots);" "ga::collect_safepoint_roots(&mut roots);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::collect_safepoint_roots(&mut roots);" "ga::collect_live_env_anchors(&mut roots);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::collect_live_env_anchors(&mut roots);" "ga::collect_live_dispatch_anchors(&mut roots);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::collect_live_dispatch_anchors(&mut roots);" "assert_rendezvous_union_complete(&roots, n);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "assert_rendezvous_union_complete(&roots, n);" "run_collection_if_triggered_rendezvous(&roots)"

# The rendezvous gate must be the witness flag, not the obsolete parked-count
# equality. The old parked-count function can survive for unit tests, but not as
# the live collection gate.
line_no "src/backend/eval/cesk/index_heap.rs" "pub fn gate_open_rendezvous() -> bool" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "crate::backend::models::gc_allocator::current_witness_ok()" >/dev/null
assert_before "src/backend/eval/cesk/index_heap.rs" "pub fn gate_open_rendezvous() -> bool" "crate::backend::models::gc_allocator::current_witness_ok()"

# R-FL free-list lifecycle: push is guarded by the persistent free bit, pop clears
# the bit before returning or discarding an entry, and released segments drain
# their listed entries before the bitmap is dropped.
assert_before "src/backend/eval/cesk/index_arena.rs" "if seg.set_free_bit(off) {" "free_list.push(addr);"
assert_before "src/backend/eval/cesk/index_arena.rs" "seg.clear_free_bit(addr.offset());" "if addr.segment() == cur {"
assert_before "src/backend/eval/cesk/index_arena.rs" "free_list.retain(|addr| {" "seg.clear_free_bit(off);"
assert_before "src/backend/eval/cesk/index_arena.rs" "drain_free_list_entries_for_released_segment(seg, &mut self.free_list, si, check);" "stats.bytes_released += seg_mut.release();"

# C1 young mark/reuse coupling: free-list reuse is current-segment-only, and the
# minor marker marks/descends only young nodes.
line_no "src/backend/eval/cesk/index_arena.rs" "pub fn pop_young_free_slot(&mut self) -> Option<Addr>" >/dev/null
line_no "src/backend/eval/cesk/index_arena.rs" "if addr.segment() == cur {" >/dev/null
line_no "src/backend/eval/cesk/index_arena.rs" "pub fn mark_young_from_roots_with" >/dev/null
line_no "src/backend/eval/cesk/index_arena.rs" "if r.segment() >= young_floor && self.mark(r) {" >/dev/null
line_no "src/backend/eval/cesk/index_arena.rs" "if k.segment() >= young_floor && self.mark(k) {" >/dev/null
assert_count "src/backend/eval/cesk/index_arena.rs" "self.cur_seg.store(" "1"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "fn open_segment(&self) -> usize {" "self.seg_count.store(idx + 1, Ordering::Release);" "self.cur_seg.store(idx, Ordering::Release);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn pop_young_free_slot(&mut self) -> Option<Addr>" "let cur = self.current_seg();" "if addr.segment() == cur {"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn pop_young_free_slot(&mut self) -> Option<Addr>" "if addr.segment() == cur {" "return Some(addr);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn try_bump_in(&self, seg: usize, node: N) -> Option<Addr>" "if seg != self.current_seg() {" "return None;"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn try_bump_in(&self, seg: usize, node: N) -> Option<Addr>" "if seg != self.current_seg() {" "Some(Addr::new(seg as u32, off as u32))"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn bump_in(&self, seg: usize, node: N) -> Addr" "bump_in target must be the current segment" "self.try_bump_in(seg, node)"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn promote_young(&self) {" "self.set_young_floor(self.current_seg());" "self.young_alloc_bytes.store(0, Ordering::Relaxed);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_sexpr(&mut self, items: &[MettaValue]) -> Addr" "let cr = self.intern_children_in(addr.segment(), items);" "self.arena.write_reused(addr, Node::SExpr(cr));"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_conjunction(&mut self, goals: &[MettaValue]) -> Addr" "let cr = self.intern_children_in(addr.segment(), goals);" "self.arena.write_reused(addr, Node::Conjunction(cr));"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "#[cfg(test)]" "*arena.get_mut(a) = TestNode::One(b);" "mod loom_model"

# Witness stamping: the stale-stamp reset and the genuine reified-park stamp are
# the only writes to published_gen. That keeps the witness theorem's
# "published implies buffered roots" premise source-grounded.
assert_count "src/backend/models/gc_allocator.rs" "published_gen.store" "2"
assert_before "src/backend/models/gc_allocator.rs" "pub(crate) fn note_reified_park(g: u64)" "slot.published_gen.store(g, Ordering::Release);"
assert_before "src/backend/models/gc_allocator.rs" "WORKER_ROOT_BUFFER.lock().extend_from_slice(roots);" "note_reified_park(my_gen);"

# V4 witness slot lifecycle: acquire before a thread is counted, release only
# after the true outermost drop count decrement, and never release at safepoint
# drops while the frozen machine is still live.
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn enter() -> Self {" "witness_acquire_slot();" "N_THREADS.fetch_add(1, Ordering::AcqRel);"
assert_after_before "src/backend/models/gc_allocator.rs" "impl Drop for EvalGuard {" "N_THREADS.fetch_sub(1, Ordering::AcqRel);" "witness_release_slot();"
assert_zero_between "src/backend/models/gc_allocator.rs" "pub fn drop_eval_guard_for_safepoint() {" "pub fn eval_guard_depth() -> u32 {" "witness_release_slot();"
assert_zero_between "src/backend/models/gc_allocator.rs" "pub fn drop_eval_guard_for_safepoint_full() -> u32 {" "pub fn reacquire_eval_guard_after_safepoint() {" "witness_release_slot();"
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn reacquire_eval_guard_after_safepoint_full(" "witness_restamp_acquired(started);" "worker_park_and_root_in_cycle(reparked_roots, started);"

# E5 started-cycle straddle gate: the driver publishes GC_CYCLE_STARTED only
# after admission is closed, before the witness wait, and the straddle loop gates
# re-parks on current_cycle_started(), not current_cycle_gen(), so teardown
# gen-bumps cannot trigger phantom re-parks.
assert_before "src/backend/eval/cesk/gc_driver.rs" "debug_assert!(" "ga::set_current_cycle_started(cur_gen);"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::set_current_cycle_started(cur_gen);" "let snap = ga::snapshot_witness(cur_gen);"
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn reacquire_eval_guard_after_safepoint_full(" "let started = current_cycle_started();" "if started > my_reparked_gen {"
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn reacquire_eval_guard_after_safepoint_full(" "if current_cycle_started() > my_reparked_gen {" "continue 'straddle;"

# E1/E5 witness-ok reset: CURRENT_WITNESS_OK is a non-generational bool, so
# cycle teardown must clear it after the gen bump and before any resume/startup
# notify can expose the next cycle. The driver must run teardown before dropping
# GC_IN_PROGRESS.
assert_after_before "src/backend/models/gc_allocator.rs" "pub(crate) fn end_rendezvous_cycle() {" "GC_CYCLE_GEN.fetch_add(1, Ordering::AcqRel);" "set_current_witness_ok(false);"
assert_after_before "src/backend/models/gc_allocator.rs" "pub(crate) fn end_rendezvous_cycle() {" "set_current_witness_ok(false);" "RENDEZVOUS_CONDVAR.notify_all();"
assert_before "src/backend/eval/cesk/gc_driver.rs" "ga::end_rendezvous_cycle();" "drop(_gip);"

echo "CESK GC source-coupling checks passed"
