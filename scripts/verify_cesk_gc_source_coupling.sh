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
  marker_line="$(line_no "$file" "$marker")" || return 1
  [[ -n "$marker_line" ]] || fail "missing marker '$marker' in $file"
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
  start_line="$(line_no "$file" "$start")" || return 1
  end_line="$(line_no_after "$file" "$start" "$end")" || return 1
  [[ -n "$start_line" ]] || fail "missing start '$start' in $file"
  [[ -n "$end_line" ]] || fail "missing end '$end' after '$start' in $file"
  awk -v start="$start_line" -v end="$end_line" -v needle="$needle" \
    'NR > start && NR < end && index($0, needle) { count++ } END { print count + 0 }' \
    "$REPO/$file"
}

assert_before() {
  local file="$1" before="$2" after="$3" before_line after_line
  before_line="$(line_no "$file" "$before")" || return 1
  after_line="$(line_no "$file" "$after")" || return 1
  [[ -n "$before_line" ]] || fail "missing '$before' in $file"
  [[ -n "$after_line" ]] || fail "missing '$after' in $file"
  if (( before_line >= after_line )); then
    fail "expected '$before' before '$after' in $file (lines $before_line >= $after_line)"
  fi
}

assert_after_before() {
  local file="$1" marker="$2" before="$3" after="$4" before_line after_line
  before_line="$(line_no_after "$file" "$marker" "$before")" || return 1
  after_line="$(line_no_after "$file" "$marker" "$after")" || return 1
  [[ -n "$before_line" ]] || fail "missing '$before' after '$marker' in $file"
  [[ -n "$after_line" ]] || fail "missing '$after' after '$marker' in $file"
  if (( before_line >= after_line )); then
    fail "expected '$before' after '$marker' and before '$after' in $file (lines $before_line >= $after_line)"
  fi
}

assert_immediate_cfg_before() {
  local file="$1" needle="$2" cfg="$3" line start found
  line="$(line_no "$file" "$needle")" || return 1
  start=$((line - 4))
  if (( start < 1 )); then
    start=1
  fi
  found="$(awk -v start="$start" -v end="$((line - 1))" -v cfg="$cfg" \
    'NR >= start && NR <= end && index($0, cfg) { print NR; exit }' \
    "$REPO/$file")"
  if [[ -z "$found" ]]; then
    fail "expected '$cfg' in the attribute window before '$needle' in $file"
  fi
}

assert_immediate_cfg_before_after() {
  local file="$1" marker="$2" needle="$3" cfg="$4" line start found
  line="$(line_no_after "$file" "$marker" "$needle")" || return 1
  start=$((line - 4))
  if (( start < 1 )); then
    start=1
  fi
  found="$(awk -v start="$start" -v end="$((line - 1))" -v cfg="$cfg" \
    'NR >= start && NR <= end && index($0, cfg) { print NR; exit }' \
    "$REPO/$file")"
  if [[ -z "$found" ]]; then
    fail "expected '$cfg' in the attribute window before '$needle' after '$marker' in $file"
  fi
}

assert_count() {
  local file="$1" needle="$2" expected="$3" actual
  actual="$(count_no "$file" "$needle")"
  if [[ "$actual" != "$expected" ]]; then
    fail "expected $expected occurrence(s) of '$needle' in $file, found $actual"
  fi
}

assert_count_between() {
  local file="$1" start="$2" end="$3" needle="$4" expected="$5" actual
  actual="$(count_between "$file" "$start" "$end" "$needle")"
  if [[ "$actual" != "$expected" ]]; then
    fail "expected $expected occurrence(s) of '$needle' between '$start' and '$end' in $file, found $actual"
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
assert_zero "src/backend/eval/cesk/index_arena.rs" "not yet wired"
assert_zero "src/backend/eval/cesk/index_heap.rs" "not yet wired"
assert_zero "src/backend/eval/cesk/index_node.rs" "not yet wired"
assert_zero "src/backend/eval/cesk/index_arena.rs" "Increment 2 is in progress"
assert_zero "src/backend/eval/cesk/index_heap.rs" "Inc 2 is in progress"
assert_zero "src/backend/eval/cesk/index_node.rs" "Inc 2 is in progress"
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

# Registry isolation: the bridge-period dynamic root registry must stay a slab
# artifact. In the index build, every value-bearing provider is reached through
# a named structural reader or a typed live-env/live-dispatch driver channel.
assert_immediate_cfg_before \
  "src/backend/models/mod.rs" \
  "pub use gc_allocator::{collect_all_roots, register_root_provider, trigger_gc_cycle, RootProvider};" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/models/gc_allocator.rs" \
  "pub trait RootProvider: Send + Sync {" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/models/gc_allocator.rs" \
  "static ROOT_REGISTRY: OnceLock<RwLock<Vec<Weak<dyn RootProvider>>>> = OnceLock::new();" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/models/gc_allocator.rs" \
  "pub fn register_root_provider(provider: &Arc<dyn RootProvider>) {" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/models/gc_allocator.rs" \
  "pub fn collect_all_roots() -> Vec<MettaValue> {" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/models/gc_allocator.rs" \
  "fn collect_all_roots_readonly() -> Vec<MettaValue> {" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/models/gc_allocator.rs" \
  "pub fn try_register_env_roots<V>(" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after \
  "src/backend/models/gc_allocator.rs" \
  "CESK A5.3: index-gc build registers ZERO providers." \
  "pub fn try_register_env_roots<V>(" \
  "#[cfg(feature = \"index-gc\")]"
assert_immediate_cfg_before \
  "src/backend/eval/mod.rs" \
  "pub(crate) mod frame_chain;" \
  "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before \
  "src/backend/eval/trampoline/mod.rs" \
  "pub(crate) mod current_iter_root;" \
  "#[cfg(not(feature = \"index-gc\"))]"

assert_immediate_cfg_before "src/backend/bytecode/cache.rs" "use crate::backend::models::{register_root_provider, RootProvider};" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/cache.rs" "struct BytecodeCacheRoots;" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/cache.rs" "impl RootProvider for BytecodeCacheRoots {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/cache.rs" "static BYTECODE_CACHE_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/cache.rs" "CESK A5.3: the index-gc build registers ZERO providers" "pub fn ensure_bytecode_cache_roots_registered() {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/cache.rs" "CESK A5.3: index-gc no-op" "pub fn ensure_bytecode_cache_roots_registered() {}" "#[cfg(feature = \"index-gc\")]"

assert_immediate_cfg_before "src/backend/bytecode/compiler/iterative.rs" "use crate::backend::models::{register_root_provider, RootProvider};" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/compiler/iterative.rs" "struct CompilerAtomRoots;" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/compiler/iterative.rs" "impl RootProvider for CompilerAtomRoots {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/compiler/iterative.rs" "static COMPILER_ATOM_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/compiler/iterative.rs" "CESK A5.3: index-gc no-op — roots are read structurally" "fn ensure_compiler_atom_roots_registered() {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/compiler/iterative.rs" "CESK A5.3: index-gc no-op (empty registry; structural roots)." "fn ensure_compiler_atom_roots_registered() {}" "#[cfg(feature = \"index-gc\")]"

assert_immediate_cfg_before "src/backend/bytecode/memo_cache.rs" "use crate::backend::models::gc_allocator::{register_root_provider, RootProvider};" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/memo_cache.rs" "struct MemoCacheRoots;" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/memo_cache.rs" "impl RootProvider for MemoCacheRoots {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/memo_cache.rs" "static MEMO_CACHE_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/memo_cache.rs" "CESK A5.3: index-gc no-op — roots are read structurally" "pub fn ensure_memo_cache_roots_registered() {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/memo_cache.rs" "CESK A5.3: index-gc no-op (empty registry; structural roots)." "pub fn ensure_memo_cache_roots_registered() {}" "#[cfg(feature = \"index-gc\")]"

assert_immediate_cfg_before "src/backend/bytecode/space_registry.rs" "use crate::backend::models::{register_root_provider, RootProvider};" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/space_registry.rs" "struct SpaceRegistryRoots;" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/space_registry.rs" "impl RootProvider for SpaceRegistryRoots {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/space_registry.rs" "static SPACE_REGISTRY_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/space_registry.rs" "CESK A5.3: index-gc no-op — roots are read structurally" "pub fn ensure_space_registry_roots_registered() {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/space_registry.rs" "CESK A5.3: index-gc no-op (empty registry; structural roots)." "pub fn ensure_space_registry_roots_registered() {}" "#[cfg(feature = \"index-gc\")]"

assert_immediate_cfg_before "src/backend/bytecode/tiered_cache.rs" "use crate::backend::models::{register_root_provider, RootProvider};" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/tiered_cache.rs" "struct TieredCacheRoots;" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/tiered_cache.rs" "impl RootProvider for TieredCacheRoots {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/bytecode/tiered_cache.rs" "static TIERED_CACHE_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/tiered_cache.rs" "CESK A5.3: index-gc no-op — roots are read structurally" "pub fn ensure_tiered_cache_roots_registered() {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before_after "src/backend/bytecode/tiered_cache.rs" "CESK A5.3: index-gc no-op (empty registry; structural roots)." "pub fn ensure_tiered_cache_roots_registered() {}" "#[cfg(feature = \"index-gc\")]"

assert_immediate_cfg_before "src/backend/environment/core.rs" "impl RootProvider for GenericEnvironmentShared<MettaValue> {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/eval/trampoline/types.rs" "impl crate::backend::models::gc_allocator::RootProvider for ParallelDispatchRootProvider {" "#[cfg(not(feature = \"index-gc\"))]"
assert_immediate_cfg_before "src/backend/eval/trampoline/types.rs" "impl crate::backend::models::gc_allocator::RootProvider for ParallelCollapseRootProvider {" "#[cfg(not(feature = \"index-gc\"))]"

# Collapse-bind binding-capture frames are metadata only. Values must keep
# travelling through BoundValue / work-item / continuation readers, not through
# a separate thread-local root source.
assert_zero "src/backend/eval/trampoline/eval_loop.rs" "capture_bindings_if_active"
assert_zero "src/backend/eval/trampoline/eval_loop.rs" "collect_binding_capture_roots"
assert_count_between "src/backend/eval/trampoline/eval_loop.rs" "struct BindingCaptureFrame {" "thread_local! {" "tracked_vars: SmallVec<[&'static str; 4]>," "1"
assert_count_between "src/backend/eval/trampoline/eval_loop.rs" "struct BindingCaptureFrame {" "thread_local! {" "collapse_fork_depth: u32," "1"
assert_zero_between "src/backend/eval/trampoline/eval_loop.rs" "struct BindingCaptureFrame {" "thread_local! {" "MettaValue"
assert_zero_between "src/backend/eval/trampoline/eval_loop.rs" "struct BindingCaptureFrame {" "thread_local! {" "BoundValue"
assert_zero_between "src/backend/eval/trampoline/eval_loop.rs" "struct BindingCaptureFrame {" "thread_local! {" "GenericBindings"
assert_zero "src/backend/eval/cesk/roots.rs" "binding_capture"

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
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn prepare_rendezvous_roots" "ga::collect_live_dispatch_anchors(&mut roots);" "assert_rendezvous_union_complete(&roots, n);"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_rendezvous_cycle" "let roots = prepare_rendezvous_roots();" "run_collection_if_triggered_rendezvous(&roots)"

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

# C1 young mark/reuse coupling: free-list reuse is current-segment-only. The
# minor marker sets mark bits only on young nodes, but traverses every reachable
# node so an old first-class SpaceHandle can expose young contents.
line_no "src/backend/eval/cesk/index_arena.rs" "pub fn pop_young_free_slot(&mut self) -> Option<Addr>" >/dev/null
line_no "src/backend/eval/cesk/index_arena.rs" "if addr.segment() == cur {" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "Node::Space(id) =>" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "self.space_handle(id).collect_gc_values(&mut values);" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "pub fn mark_young(&self, roots: &[Addr]) -> usize" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "let mut seen = std::collections::HashSet::with_capacity" >/dev/null
line_no "src/backend/eval/cesk/index_heap.rs" "if addr.segment() >= young_floor && arena.mark(addr) {" >/dev/null
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
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" "Node::Space(id) =>" "self.space_handle(id).collect_gc_values(&mut values);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn mark_young(&self, roots: &[Addr]) -> usize" "for &root in roots" "while let Some(addr) = worklist.pop()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn mark_young(&self, roots: &[Addr]) -> usize" "self.child_addrs_for_mark(addr, &mut kids);" "for &child in &kids"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "self.child_addrs_for_mark(addr, &mut kids);" "for &child in &kids" "marked += enqueue("
assert_after_before "src/backend/eval/cesk/index_arena.rs" "#[cfg(test)]" "*arena.get_mut(a) = TestNode::One(b);" "mod loom_model"

# Node-edge completeness: the marker's concrete edge reader must cover every
# semantic Addr-bearing edge class in `Node`: inline handle fields,
# SExpr/Conjunction side-arena children, and first-class SpaceHandle contents.
# State nodes are ids whose cell values are rooted by the structural env reader;
# Memo nodes store serialized bytes, not live MettaValues.
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" "node.child_addrs(out);" "match *node"
assert_after_before "src/backend/eval/cesk/index_node.rs" "fn child_addrs(&self, out: &mut Vec<Addr>)" "Node::Error(a, b)" "push(a, out);"
assert_after_before "src/backend/eval/cesk/index_node.rs" "Node::Error(a, b)" "push(a, out);" "push(b, out);"
assert_after_before "src/backend/eval/cesk/index_node.rs" "fn child_addrs(&self, out: &mut Vec<Addr>)" "Node::Type(a) | Node::Quoted(a) | Node::Lazy(a) => push(a, out)" "Node::Spanned(a, _) => push(a, out)"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" "Node::SExpr(cr) | Node::Conjunction(cr) =>" "side.children.get(cr.idx)"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" "for c in kids.iter()" "if let Some(a) = c.as_arena_addr()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" "Node::Space(id) =>" "self.space_handle(id).collect_gc_values(&mut values);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "Node::Space(id) =>" "self.space_handle(id).collect_gc_values(&mut values);" "if let Some(a) = value.as_arena_addr()"
assert_after_before "src/backend/environment/core.rs" "pub(crate) fn collect_roots_into(&self, roots: &mut Vec<MettaValue>)" "let states = self.states.read();" "roots.extend(states.values().copied());"
assert_after_before "src/backend/models/memo_handle.rs" "struct MemoEntry" "results_bytes: Vec<Vec<u8>>" "}"
assert_zero_between "src/backend/models/memo_handle.rs" "struct MemoEntry" "impl MemoHandle" "MettaValue"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn child_addrs_for_mark(&self, addr: Addr, out: &mut Vec<Addr>)" "Node::State(_)" "Node::Memo(_)"

# B2'/D2 source-channel registration: the driver-root-union proof only applies
# if live envs and parallel fan-outs are registered for their lifetimes and the
# registry walkers delegate to the structural root readers.
assert_count "src/backend/eval/mod.rs" "register_live_env(" "1"
assert_count "src/backend/eval/trampoline/eval_loop.rs" "register_live_env(&dyn_env)" "2"
assert_after_before "src/backend/eval/mod.rs" "let _live_env_handle = {" "register_live_env(" "let r = eval_inner(value, env, state);"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "Register THIS worker's branch env" "register_live_env(&dyn_env)" "eval_trampoline_with_carrying(branch_expr, env"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "THIS collapse worker's env" "register_live_env(&dyn_env)" "eval_trampoline_with_carrying("
assert_after_before "src/backend/environment/core.rs" "impl crate::backend::models::gc_allocator::EnvRoots for GenericEnvironmentShared<MettaValue>" "self.collect_roots_into(out);" "#[cfg(not(feature = \"index-gc\"))]"
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn collect_live_env_anchors(out: &mut Vec<MettaValue>)" "weak.upgrade()" "strong.collect_env_roots(out);"

assert_after_before "src/backend/eval/mod.rs" "pub fn eval(" "state.collect_driver_program_roots(&mut driver_roots);" "let _guard = EvalGuard::enter();"
assert_after_before "src/backend/eval/mod.rs" "pub fn eval(" "crate::backend::models::register_temporary_roots(driver_roots)" "let _guard = EvalGuard::enter();"
assert_after_before "src/backend/eval/tier_forced.rs" "pub fn eval_with_tier(" "state.collect_driver_program_roots(&mut driver_roots);" "let outcome = if let Err(reason) = tier_applicable"
assert_after_before "src/backend/eval/tier_forced.rs" "pub fn eval_with_tier(" "crate::backend::models::register_temporary_roots(driver_roots)" "let outcome = if let Err(reason) = tier_applicable"

# E2 batch-result handoff coupling: async rholang batch workers leave the
# rendezvous participant set before the caller consumes their result vectors.
# Each BatchOutcome must therefore carry a persistent safepoint root handle from
# before publication into the gather slot until after the caller copies the
# results into MettaState.output.
assert_after_before "src/rholang_integration.rs" "struct BatchOutcome" "_root_handle: Option<crate::backend::models::SafepointRootHandle>" "}"
assert_after_before "src/rholang_integration.rs" "let result_vec: Vec<MettaValue> =" "eval_results.into_iter().map(|(value, _bindings)| value).collect();" "register_temporary_roots("
assert_after_before "src/rholang_integration.rs" "let result_vec: Vec<MettaValue> =" "register_temporary_roots(" "guard[slot] = Some(BatchOutcome"
assert_after_before "src/rholang_integration.rs" "let result_vec: Vec<MettaValue> =" "register_temporary_roots(" "_root_handle: root_handle,"
assert_after_before "src/rholang_integration.rs" "guard[slot] = Some(BatchOutcome" "_root_handle: root_handle," "remaining.fetch_sub"
assert_after_before "src/rholang_integration.rs" "let mut collected: Vec<BatchOutcome>" "drain(..)" "collected.sort_by_key"
assert_zero "src/rholang_integration.rs" "drop(root_handle)"
assert_count "src/rholang_integration.rs" "let batch_results = evaluate_batch_parallel_arena(current_batch, env.clone()).await;" "2"
assert_count "src/rholang_integration.rs" "for outcome in batch_results {" "2"
assert_count_between "src/rholang_integration.rs" "if (is_rule_def || is_ground_fact) && !current_batch.is_empty() {" "current_batch = Vec::new();" "for outcome in batch_results {" "1"
assert_after_before "src/rholang_integration.rs" "if (is_rule_def || is_ground_fact) && !current_batch.is_empty() {" "output.push(result);" "(and its index-gc _root_handle) drops HERE"
assert_count_between "src/rholang_integration.rs" "// Evaluate any remaining batch" "// Transfer final environment to result state" "for outcome in batch_results {" "1"
assert_after_before "src/rholang_integration.rs" "// Evaluate any remaining batch" "output.push(result);" "(+ its index-gc _root_handle) drops HERE"

assert_count "src/backend/eval/trampoline/eval_loop.rs" "register_live_dispatch(" "2"
assert_count "src/backend/eval/trampoline/eval_loop.rs" "_live_dispatch: live_dispatch" "2"
assert_count "src/backend/eval/trampoline/types.rs" "pub(crate) _live_dispatch: Option<crate::backend::models::gc_allocator::LiveDispatchHandle>" "2"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "let live_dispatch = if crate::backend::models::gc_allocator::dedicated_gc_enabled() {" "register_live_dispatch(" "_live_dispatch: live_dispatch,"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "register the collapse fan-out" "register_live_dispatch(" "_live_dispatch: live_dispatch,"
assert_after_before "src/backend/eval/trampoline/types.rs" "impl crate::backend::models::gc_allocator::DispatchRoots for ParallelDispatchRootProvider" "for (value, bindings) in self.branches.iter()" "if let Ok(guard) = self.results.try_lock()"
assert_after_before "src/backend/eval/trampoline/types.rs" "impl crate::backend::models::gc_allocator::DispatchRoots for ParallelCollapseRootProvider" "for (value, bindings) in self.items.iter()" "if let Ok(guard) = self.results.try_lock()"
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn collect_live_dispatch_anchors(out: &mut Vec<MettaValue>)" "weak.upgrade()" "strong.collect_dispatch_roots(out);"
assert_after_before "src/backend/models/gc_allocator.rs" "pub fn snapshot_live_dispatch_witness() -> (Vec<MettaValue>, usize)" "weak.upgrade()" "strong.collect_dispatch_roots(&mut out);"

# E1 parallel completion coupling: the CollapseCompletion proof/TLA model only
# applies if every spawned dispatch/collapse worker owns one RAII completion
# guard, the sole executable decrement is in that guard's Drop, and the parent
# waits observe completion by the remaining counter reaching zero.
assert_count "src/backend/eval/trampoline/eval_loop.rs" "let _completion = CompletionGuard {" "2"
assert_count "src/backend/eval/trampoline/eval_loop.rs" "if self.remaining.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {" "1"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "impl Drop for CompletionGuard" "self.remaining.fetch_sub(1, std::sync::atomic::Ordering::AcqRel)" "cvar.notify_one();"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_dispatch(" "let closure = move || {" "let _completion = CompletionGuard {"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_dispatch(" "let _completion = CompletionGuard {" "PARALLEL_BRANCH_DEPTH.with(|d| d.set(child_depth));"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_dispatch(" "let _completion = CompletionGuard {" "let _guard = EvalGuard::enter();"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_collapse_dispatch(" "let closure = move || {" "let _completion = CompletionGuard {"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_collapse_dispatch(" "let _completion = CompletionGuard {" "PARALLEL_BRANCH_DEPTH.with(|d| d.set(child_depth));"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_collapse_dispatch(" "let _completion = CompletionGuard {" "let _guard = EvalGuard::enter();"
assert_count "src/backend/eval/trampoline/eval_loop.rs" "handle.remaining.load(Ordering::Acquire) == 0 || handle.cancel_token.is_satisfied();" "2"

# E1 worker admission coupling: the WorkerAdmission proof/TLA model only applies
# if the driver closes admission before its participant snapshot, ordinary
# EvalGuard entry backs out while GC_IN_PROGRESS is set before joining the
# counted thread set, and spawned dispatch/collapse workers take the early
# dedicated-GC admission wait before EvalGuard::enter.
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_rendezvous_cycle" "let _gip = acquire_gc_in_progress_for_rendezvous();" "let roots = prepare_rendezvous_roots();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_stw_rendezvous_cycle" "let gip = acquire_gc_in_progress_for_rendezvous();" "let roots = prepare_rendezvous_roots();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn prepare_rendezvous_roots" "let n = ga::n_threads();" "ga::set_n_threads_at_snapshot(n);"
assert_after_before "src/backend/models/gc_allocator.rs" "impl EvalGuard" "ACTIVE_EVALUATORS.fetch_add(1, Ordering::AcqRel);" "if !GC_IN_PROGRESS.load(Ordering::Acquire) {"
assert_after_before "src/backend/models/gc_allocator.rs" "impl EvalGuard" "ACTIVE_EVALUATORS.fetch_sub(1, Ordering::AcqRel);" "let mut lock = GC_PROGRESS_MUTEX.lock();"
assert_after_before "src/backend/models/gc_allocator.rs" "impl EvalGuard" "if !GC_IN_PROGRESS.load(Ordering::Acquire) {" "EVAL_GUARD_DEPTH.with"
assert_after_before "src/backend/models/gc_allocator.rs" "impl EvalGuard" "EVAL_GUARD_DEPTH.with" "N_THREADS.fetch_add(1, Ordering::AcqRel);"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_dispatch(" "if crate::backend::models::gc_allocator::dedicated_gc_enabled() {" "crate::backend::models::gc_allocator::worker_wait_for_resume();"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_dispatch(" "crate::backend::models::gc_allocator::worker_wait_for_resume();" "let _guard = EvalGuard::enter();"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_collapse_dispatch(" "if crate::backend::models::gc_allocator::dedicated_gc_enabled() {" "crate::backend::models::gc_allocator::worker_wait_for_resume();"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "fn parallel_collapse_dispatch(" "crate::backend::models::gc_allocator::worker_wait_for_resume();" "let _guard = EvalGuard::enter();"

# E1 self-root publication coupling: the ThreadContribution formal obligation
# only applies if the single canonical reader contains every component it claims.
# Trampoline participants publish extra hot values, S/C/K with live-K narrowing,
# E0, global anchors, K-spine, and deferred env roots. Tier leaves publish extra
# VM/JIT values plus the env-less persistent roots they can read locally.
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_machine_roots(" "rs.collect_all(operand_stack, current_work, work_stack, continuations);" "collect_persistent_roots(out, env0);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_machine_roots_live(" "rs.collect_all_live(operand_stack, current_work, work_stack, continuations);" "collect_persistent_roots(out, env0);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_persistent_roots_no_env0" "collect_global_anchors(out);" "super::k_spine::collect_k_spine(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_persistent_roots(" "env0.collect_roots_into(out);" "collect_global_anchors(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_persistent_roots(" "collect_global_anchors(out);" "super::k_spine::collect_k_spine(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "ThreadContribution::Trampoline {" "out.extend_from_slice(extra);" "collect_machine_roots_live("
assert_after_before "src/backend/eval/cesk/roots.rs" "ThreadContribution::Trampoline {" "collect_machine_roots_live(" "for e in deferred_envs {"
assert_after_before "src/backend/eval/cesk/roots.rs" "ThreadContribution::Trampoline {" "for e in deferred_envs {" "e.as_ref().collect_roots_into(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "ThreadContribution::TierLeaf { extra }" "out.extend_from_slice(extra);" "collect_persistent_roots_no_env0(out);"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "pub(crate) fn worker_cooperative_safepoint" "ThreadContribution::TierLeaf" "gc_allocator::worker_park_and_root_in_cycle"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "FULL park (mirror branch-B template" "ThreadContribution::Trampoline" "worker_park_and_root_in_cycle"

# Tier-leaf VM/JIT extra roots: the TierLeafExtraRoots proof/TLA model applies
# only if the concrete VM and JIT readers enumerate every tier-local
# value-bearing field before the worker publishes a TierLeaf contribution.
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "super::cache::collect_generic_chunk_constants(&self.chunk, out);" "out.extend(self.value_stack.iter().cloned());"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "out.extend(self.value_stack.iter().cloned());" "out.extend(self.locals.iter().cloned());"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "out.extend(self.locals.iter().cloned());" "out.extend(self.results.iter().cloned());"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "out.extend(self.results.iter().cloned());" "if let Some(ref et) = self.expected_type"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "for (_scope, _name, val) in self.current_bindings.iter_full()" "for frame in self.bindings_stack.iter()"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "for frame in self.call_stack.iter()" "for cp in self.choice_points.iter()"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "for cp in self.choice_points.iter()" "for frame in self.collapse_frames.iter()"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "for frame in self.collapse_frames.iter()" "for frame in self.collapse_bind_frames.iter()"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "for frame in self.collapse_bind_frames.iter()" "for bindings in self.per_result_bindings.iter()"
assert_after_before "src/backend/bytecode/vm/mod.rs" "pub(crate) fn collect_roots_into" "for (_, vs) in self.dispatch_memo.values()" "for entry in self.trail.iter()"
assert_after_before "src/backend/bytecode/vm/mod.rs" "fn run_cooperative_safepoint" "self.collect_roots_into(&mut buf);" "worker_cooperative_safepoint(&buf_mv);"
assert_after_before "src/backend/bytecode/vm/mod.rs" "periodic cooperative GC safepoint for parallel-" "cfg!(feature = \"index-gc\")" "self.run_cooperative_safepoint();"
assert_after_before "src/backend/eval/cesk/k_spine.rs" "VmLeaf::Vm { vm }" "(*vm).collect_roots_into(out);" "VmLeaf::SavedBindings"
assert_after_before "src/backend/eval/cesk/k_spine.rs" "VmLeaf::Jit { ctx }" "collect_jit_roots_into(" "&*ctx, out,"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "collect_constant_array_roots(ctx.constants" "ctx.arena_constants as *const MettaValue"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "ctx.arena_constants as *const MettaValue" "collect_chunk_ptr_constants(ctx.current_chunk, out);"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "ctx.value_stack.add(i)" "ctx.results.add(i)"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "ctx.results.add(i)" "ctx.saved_stack.add(i)"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "ctx.saved_stack.add(i)" "ctx.choice_points.add(i)"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "ctx.choice_points.add(i)" "ctx.binding_frames.add(i)"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "entry.value.0" "ctx.template_results.add(i)"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_roots_into" "ctx.template_results.add(i)" "ctx.state_cache_valid"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_value_into" "gc_mode_is_index()" "Addr::from_raw"
assert_after_before "src/backend/bytecode/jit/runtime/gc_roots.rs" "pub(crate) unsafe fn collect_jit_value_into" "Addr::from_raw" "MettaValue::from_addr"
assert_count "src/backend/bytecode/jit/runtime/call_support.rs" "roots.push(arg.clone());" "2"
assert_after_before "src/backend/bytecode/jit/runtime/call_support.rs" "roots.push(arg.clone());" "collect_jit_roots_into" "worker_cooperative_safepoint(&roots);"
assert_after_before "src/backend/bytecode/jit/runtime/sexpr_ops.rs" "unsafe fn jit_maybe_pre_eval_structural" "roots.push(v.clone());" "collect_jit_roots_into"
assert_after_before "src/backend/bytecode/jit/runtime/sexpr_ops.rs" "unsafe fn jit_maybe_pre_eval_structural" "collect_jit_roots_into" "worker_cooperative_safepoint(&roots);"
assert_after_before "src/backend/bytecode/jit/hybrid/arena.rs" "pub fn execute_jit_arena_direct" "VmLeaf::Jit" "native_fn(&mut ctx);"
assert_after_before "src/backend/bytecode/jit/hybrid/arena.rs" "pub fn execute_jit_arena_with_env" "VmLeaf::Jit" "native_fn(&mut ctx);"

# Forked-env frame roots: fork_for_nondeterminism deep-copies the five
# Addr-bearing local maps, and every index-mode live work item / continuation
# frame must include those maps in its structural K roots. The Arc-shared E0
# registries stay at the persistent-root level and are not rewalked per frame.
assert_after_before "src/backend/environment/core.rs" "pub fn fork_for_nondeterminism(&self) -> Self" "states: RwLock::new(self.shared.states.read().clone())," "bindings: RwLock::new(self.shared.bindings.read().clone()),"
assert_after_before "src/backend/environment/core.rs" "pub fn fork_for_nondeterminism(&self) -> Self" "named_spaces: RwLock::new(self.shared.named_spaces.read().clone())," "bindings: RwLock::new(self.shared.bindings.read().clone()),"
assert_after_before "src/backend/environment/core.rs" "pub fn fork_for_nondeterminism(&self) -> Self" "bindings: RwLock::new(self.shared.bindings.read().clone())," "types: RwLock::new(self.shared.types.read().clone()),"
assert_after_before "src/backend/environment/core.rs" "pub fn fork_for_nondeterminism(&self) -> Self" "types: RwLock::new(self.shared.types.read().clone())," "inferred_fn_types: DashMap::from_iter("
assert_after_before "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "out.extend(s.bindings.read().values().copied());" "for tv in s.types.read().values()"
assert_after_before "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "for tv in s.types.read().values()" "out.extend(tv.iter().copied());"
assert_after_before "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "out.extend(s.states.read().values().copied());" "for (_name, atoms) in s.named_spaces.read().values()"
assert_after_before "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "for (_name, atoms) in s.named_spaces.read().values()" "out.extend(atoms.iter().copied());"
assert_after_before "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "for entry in s.inferred_fn_types.iter()" "out.extend(entry.value().iter().copied());"
assert_count_between "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "impl WorkItem {" "collect_roots_into" "0"
assert_count_between "src/backend/eval/trampoline/types.rs" "fn collect_fork_local_roots" "impl WorkItem {" "atom_space.collect_gc_roots" "0"
assert_count_between "src/backend/eval/trampoline/types.rs" "impl WorkItem {" "pub fn collect_values(&self, out: &mut Vec<MettaValue>)" "_ =>" "0"
assert_after_before "src/backend/eval/trampoline/types.rs" "impl WorkItem {" "if let Some(e) = self.frame_env()" "collect_fork_local_roots(e, out);"
assert_count_between "src/backend/eval/trampoline/types.rs" "impl Continuation {" "pub fn collect_values(&self, out: &mut Vec<MettaValue>)" "_ =>" "0"
assert_after_before "src/backend/eval/trampoline/types.rs" "impl Continuation {" "if let Some(e) = self.frame_env()" "collect_fork_local_roots(e, out);"
assert_count_between "src/backend/eval/trampoline/types.rs" "pub fn collect_live_values(&self, out: &mut Vec<MettaValue>)" "pub fn depth_hint(&self) -> usize" "collect_fork_local_roots(env, out);" "3"
assert_after_before "src/backend/eval/trampoline/types.rs" "pub fn collect_live_values(&self, out: &mut Vec<MettaValue>)" "_ => self.collect_values(out)," "pub fn depth_hint(&self) -> usize"

# Single-threaded mid-loop collection coupling: the opt-in mid-loop branch must
# build exactly the root union discharged by MidloopRootUnion before handing it
# to the collector. This is the live trampoline S/C/K reader (which also appends
# E0/global/K-spine), then deferred env drops, then driver-C safepoint roots.
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn gate_open_midloop() -> bool" "gc_mode_is_index()" "&& midloop_enabled()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn gate_open_midloop() -> bool" "&& midloop_enabled()" "&& !worker_ever_spawned()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn gate_open_midloop() -> bool" "&& !worker_ever_spawned()" "&& active_evaluator_count() == 1"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn gate_open_midloop() -> bool" "&& active_evaluator_count() == 1" "&& !disabled()"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "let mut midloop_roots" "collect_machine_roots_live("
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "collect_machine_roots_live(" "for deferred_env in &deferred_shared_drops"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "deferred_env.as_ref().collect_roots_into(&mut midloop_roots);" "crate::backend::models::collect_safepoint_roots(&mut midloop_roots);"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "crate::backend::models::collect_safepoint_roots(&mut midloop_roots);" "run_collection_if_triggered_midloop("
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "run_collection_if_triggered_midloop(" "&midloop_roots,"
assert_count_between "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "// Phase 2.2: Incremental nursery collection" "collect_machine_roots_live(" "1"
assert_count_between "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "// Phase 2.2: Incremental nursery collection" "deferred_env.as_ref().collect_roots_into(&mut midloop_roots);" "1"
assert_count_between "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "// Phase 2.2: Incremental nursery collection" "crate::backend::models::collect_safepoint_roots(&mut midloop_roots);" "1"
assert_count_between "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "// Phase 2.2: Incremental nursery collection" "run_collection_if_triggered_midloop(" "1"
assert_zero_between "src/backend/eval/trampoline/eval_loop.rs" "} else if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {" "// Phase 2.2: Incremental nursery collection" "crate::backend::models::collect_all_roots()"
assert_after_before "src/backend/eval/trampoline/types.rs" "\`remaining_matches\` is dead once the cut fired for this barrier." "if !cut_fired_peek(*cut_barrier) {" "remaining_matches.as_slice()"
assert_after_before "src/backend/eval/trampoline/types.rs" "\`remaining_alts\` is dead once the cut fired for this barrier." "if !cut_fired_peek(*cut_barrier) {" "remaining_alts.as_slice()"
assert_after_before "src/backend/eval/trampoline/types.rs" "\`remaining_templates\` is dead once the cut fired for this barrier." "if !cut_fired_peek(*cut_barrier) {" "remaining_templates.as_slice()"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "Continuation::ProcessRuleMatches {" "if remaining_matches.len() == 0 || cut_fired {" "let (rhs, raw_bindings) = remaining_matches"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "D-2 (C2) soundness coupling: \`collect_live_values\` SKIPS \`remaining_matches\`" "debug_assert!(" "let (rhs, raw_bindings) = remaining_matches"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "Phase 1 cut-barrier: if a \`(cut)\` fired this disjunction's" "if cut_fired_peek(cut_barrier) {" "remaining_alts.next()"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "D-2 (C2) soundness coupling: \`collect_live_values\` skips \`remaining_alts\`" "debug_assert!(" "remaining_alts.next()"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "Phase 1 cut-barrier: if a \`(cut)\` fired this match fan-out's" "if cut_fired_peek(cut_barrier) {" "remaining_templates.next()"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "D-2 (C2) soundness coupling: \`collect_live_values\` skips \`remaining_templates\`" "debug_assert!(" "remaining_templates.next()"

# E2 cache-epoch source coupling: OPERATOR_CACHE is pointer-keyed
# (`head.as_ptr()`), so index mode must lazily clear it when gc_sweep_epoch
# advances on a different thread. Explicit cache clears also synchronize the
# local epoch to avoid a redundant clear on the same epoch.
assert_before "src/backend/eval/trampoline/dispatch_hints.rs" "static OPERATOR_CACHE_GC_EPOCH:" "fn ensure_operator_cache_gc_epoch_current()"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "fn ensure_operator_cache_gc_epoch_current()" "gc_sweep_epoch()" "OPERATOR_CACHE.with"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "fn ensure_operator_cache_gc_epoch_current()" "cache_cell.borrow_mut().clear();" "e.set(current);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn operator_cache_get" "ensure_operator_cache_gc_epoch_current();" "let current_epoch = RULE_EPOCH.load(Ordering::Acquire);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_operator_cache()" "cache_cell.borrow_mut().clear();" "OPERATOR_CACHE_GC_EPOCH.with"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "Keep explicit clears coherent with the lazy sweep-epoch guard." "gc_sweep_epoch()" "});"

# E2 SATB LRU source coupling: value-bearing E0 anchor caches must not use
# `LruCache::put` for index-mode eviction paths, because `put` hides capacity
# victims. The source must use `push`, then shade the surfaced victim. Every
# value-dropping E0 cache operation must run under the SATB phase gate so marker
# start cannot straddle a deletion that observed "not marking".
line_no "src/backend/eval/cesk/index_heap.rs" "static SATB_PHASE_LOCK: RwLock<()> = RwLock::new(());" >/dev/null
assert_before "src/backend/eval/cesk/index_heap.rs" "static SATB_PHASE_LOCK" "static SATB_MARKING_DEPTH"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn satb_marking_in_progress" "SATB_MARKING_DEPTH.load(Ordering::Acquire)" "depth > 0"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn with_satb_deletion_barrier" "SATB_PHASE_LOCK.read()" "f(satb_marking_in_progress())"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn enter_satb_marking" "SATB_PHASE_LOCK.write()" "SATB_MARKING_DEPTH.fetch_add(1, Ordering::AcqRel);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "impl Drop for SatbMarkingGuard" "SATB_PHASE_LOCK.write()" "SATB_MARKING_DEPTH.fetch_sub(1, Ordering::AcqRel);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "impl Drop for SatbMarkingGuard" "SATB_MARKING_DEPTH.fetch_sub(1, Ordering::AcqRel);" "debug_assert!(prev > 0"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_rendezvous_cycle" "concurrent_satb_enabled()" "gc_driver_satb_rendezvous_cycle"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_rendezvous_cycle" "let satb_result = std::panic::catch_unwind" "if satb_result.is_err()"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_rendezvous_cycle" "if satb_result.is_err()" "gc_driver_stw_rendezvous_cycle();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_stw_rendezvous_cycle" "request_gc();" "let gip = acquire_gc_in_progress_for_rendezvous();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_stw_rendezvous_cycle" "let gip = acquire_gc_in_progress_for_rendezvous();" "let roots = prepare_rendezvous_roots();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_stw_rendezvous_cycle" "let roots = prepare_rendezvous_roots();" "run_open_stw_rendezvous_cycle(roots, gip);"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn run_open_stw_rendezvous_cycle" "run_collection_if_triggered_rendezvous(&roots)" "drop(roots);"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn run_open_stw_rendezvous_cycle" "drop(roots);" "close_open_rendezvous_cycle(Some(gip));"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "enter_satb_marking()" "cleanup.close_cycle();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "cleanup.close_cycle();" "mark_concurrent_roots"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "mark_concurrent_roots" "cleanup.request_next_cycle();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "cleanup.request_next_cycle();" "let final_roots = prepare_rendezvous_roots();"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "drop(satb_guard);" "sweep_after_concurrent_mark"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "let swept = crate::backend::eval::cesk::index_heap::index_gc::sweep_after_concurrent_mark" "assert!("
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn gc_driver_satb_rendezvous_cycle" "assert!(" "drop(final_roots);"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "impl Drop for SatbRendezvousCleanup" "if self.cycle_open" "close_open_rendezvous_cycle(self.gip.take());"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "impl Drop for SatbRendezvousCleanup" "else if self.request_open" "resume_workers();"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn satb_shade_evicted_roots" "gc_mode_is_index()" "let mut addrs"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn satb_shade_evicted_roots" "!satb_marking_in_progress()" "let mut addrs"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn satb_shade_evicted_roots" "global_index_heap().read().expect" "heap.mark(&addrs);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn mark_concurrent_roots" "global_index_heap().read().expect" "heap.mark_concurrent(&addrs)"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn mark_concurrent" "self.mark(roots)" "}"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "fn open_segment(&self) -> usize" "(*self.segments[idx].get()).write(seg);" "self.seg_count.store(idx + 1, Ordering::Release);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "fn open_segment(&self) -> usize" "self.seg_count.store(idx + 1, Ordering::Release);" "self.cur_seg.store(idx, Ordering::Release);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "unsafe fn segment(&self, i: usize) -> &Segment<N>" "self.segments[i].get()" "assume_init_ref()"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn alloc_bump(&self, node: N) -> Addr" "let seg = unsafe { self.segment(si) };" "seg.write_claimed_allocate_black(off, node)"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn alloc_bump(&self, node: N) -> Addr" "seg.write_claimed_allocate_black(off, node)" "seg.publish(off);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn alloc_bump(&self, node: N) -> Addr" "seg.publish(off);" "return Addr::new(si as u32, off as u32);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn get(&self, addr: Addr) -> &N" "let seg = self.segment(addr.segment());" "addr.offset() < seg.len.load(Ordering::Acquire)"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn get(&self, addr: Addr) -> &N" "addr.offset() < seg.len.load(Ordering::Acquire)" "seg.node_at(addr.offset())"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "unsafe fn node_at(&self, offset: usize) -> &N" "self.nodes[offset].get()" "assume_init_ref()"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "unsafe fn write_claimed_allocate_black" "self.write_claimed(off, node);" "satb_marking_in_progress()"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "unsafe fn write_claimed_allocate_black" "satb_marking_in_progress()" "self.set_mark(off);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn alloc_bump" "seg.write_claimed_allocate_black(off, node)" "seg.publish(off);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn try_bump_in" "s.write_claimed_allocate_black(off, node)" "s.publish(off);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn try_bump_in(&self, seg: usize, node: N) -> Option<Addr>" "let s = unsafe { self.segment(seg) };" "s.write_claimed_allocate_black(off, node)"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn try_bump_in(&self, seg: usize, node: N) -> Option<Addr>" "s.write_claimed_allocate_black(off, node)" "s.publish(off);"
assert_after_before "src/backend/eval/cesk/index_arena.rs" "pub fn try_bump_in(&self, seg: usize, node: N) -> Option<Addr>" "s.publish(off);" "Some(Addr::new(seg as u32, off as u32))"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn ensure_side_seg(&self, seg: usize)" "(*self.sides[next].get()).write(arenas);" "self.sides_count.store(next + 1, Ordering::Release);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn intern_children_in" "self.ensure_side_seg(seg);" "let side = unsafe { self.side(seg) };"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn intern_bytes_in" "self.ensure_side_seg(seg);" "let side = unsafe { self.side(seg) };"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn intern_span_in" "self.ensure_side_seg(seg);" "let side = unsafe { self.side(seg) };"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn grow_to(&self, c: usize)" "(*self.pages[p].get()).write(page);" "self.page_count.store(p + 1, Ordering::Release);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn grow_to(&self, c: usize)" "self.page_count.store(p + 1, Ordering::Release);" "(*page[ck].get()).write(chunk);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn grow_to(&self, c: usize)" "(*page[ck].get()).write(chunk);" "self.chunk_count.store(next + 1, Ordering::Release);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn push(&self, boxed: Box<T>) -> u32" "self.grow_to(c);" "let chunk = self.chunk(c);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn push(&self, boxed: Box<T>) -> u32" "(*chunk[off].get()).write(Some(boxed));" "self.publish(idx);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "unsafe fn get(&self, idx: u32) -> Option<&T>" "self.len.load(Ordering::Acquire)" "let chunk = self.chunk(c);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_sexpr(&mut self, items: &[MettaValue]) -> Addr" "let cs = self.intern_children_in(seg, items);" "self.arena.try_bump_in(seg, Node::SExpr(cs))"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_conjunction(&mut self, goals: &[MettaValue]) -> Addr" "let cs = self.intern_children_in(seg, goals);" "self.arena.try_bump_in(seg, Node::Conjunction(cs))"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_atom(&mut self, s: &str) -> Addr" "let bs = self.intern_bytes_in(seg, s);" "self.arena.try_bump_in(seg, Node::Atom(bs))"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_string(&mut self, s: &str) -> Addr" "let bs = self.intern_bytes_in(seg, s);" "self.arena.try_bump_in(seg, Node::String(bs))"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn alloc_spanned(&mut self, inner: MettaValue, span: Span) -> Addr" "let sr = self.intern_span_in(seg, span);" "self.arena.try_bump_in(seg, Node::Spanned(inner, sr))"

# Side-payload free quiescence coupling: dropping side-arena Boxes is separated
# from reclaiming fixed node slots. It may happen only on the true-quiescence
# phase, and the materialization shadow is cleared before the routine returns to
# code that can dereference materialized inners again.
line_no "src/backend/eval/cesk/index_heap.rs" "gc_mode_is_index() && !worker_ever_spawned() && active_evaluator_count() == 0 && !disabled()" >/dev/null
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn run_collection_if_triggered(roots: &[MettaValue]) -> bool" "gate_open()" "mark_sweep_if_over_watermark(roots, \"quiescence\")"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn run_collection_if_triggered_rendezvous" "gate_open_rendezvous()" "mark_sweep_if_over_watermark(roots, \"rendezvous\")"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn run_collection_if_triggered_midloop" "gate_open_midloop()" "mark_sweep_if_over_watermark(roots, \"midloop\")"
assert_count "src/backend/eval/cesk/index_heap.rs" "heap.free_reclaimed_side_slots(&reclaimed);" "2"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "if phase == \"quiescence\"" "heap.free_reclaimed_side_slots(&reclaimed);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn mark_sweep_if_over_watermark" "if phase == \"quiescence\"" "heap.free_reclaimed_side_slots(&reclaimed);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "heap.free_reclaimed_side_slots(&reclaimed);" "clear_inner_shadow();"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn mark_sweep_if_over_watermark" "heap.free_reclaimed_side_slots(&reclaimed);" "clear_inner_shadow();"

# Addr-valued hash-cons entries must not survive a sweep when their slot is
# about to be reclaimed. Lookup also revalidates the existing node's child
# handles before returning a table hit.
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn intern_ground_sexpr(&mut self, items: &[MettaValue]) -> MettaValue" "self.hash_cons.get(&key)" "existing.as_arena_addr()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn intern_ground_sexpr(&mut self, items: &[MettaValue]) -> MettaValue" "let kids = self.children(addr);" "kids.len() == items.len()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn intern_ground_sexpr(&mut self, items: &[MettaValue]) -> MettaValue" "kids.iter().zip(items).all(|(a, b)| a.tagged == b.tagged)" "return existing;"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn sweep(&mut self) -> SweepStats" ".retain(|_, v| v.as_arena_addr().is_some_and(|a| arena.is_marked(a)));" "self.arena.sweep_with("
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn sweep_young(&mut self) -> SweepStats" "let young_floor = self.arena.young_floor();" "self.hash_cons.retain"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn sweep_young(&mut self) -> SweepStats" "Some(a) if a.segment() >= young_floor => arena.is_marked(a)," "Some(_) => true,"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn sweep_young(&mut self) -> SweepStats" "Some(_) => true," "None => false,"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub fn sweep_young(&mut self) -> SweepStats" "self.hash_cons.retain" "self.arena.sweep_young_with("
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "global_index_heap().write().expect(\"index heap\")" "heap.mark(&addrs);"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "heap.mark(&addrs);" "let stats = heap.sweep();"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "heap.sweep();" "heap.promote_young();"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "heap.promote_young();" "MAJOR_CYCLES_RUN.fetch_add"
assert_zero_between "src/backend/eval/cesk/index_heap.rs" "pub(crate) fn sweep_after_concurrent_mark" "MAJOR_CYCLES_RUN.fetch_add" "sweep_young"
assert_after_before "src/backend/eval/trampoline/eval_loop.rs" "(A) FANOUT>0 WATERMARK TRIGGER" "!crate::backend::eval::cesk::index_heap::index_gc::satb_marking_in_progress()" "watermark_due_for_concurrent()"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn mark_sweep_if_over_watermark" "GcInProgressGuard::try_enter();" "global_index_heap().write().expect(\"index heap\")"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn mark_sweep_if_over_watermark" "global_index_heap().write().expect(\"index heap\")" "heap.mark(&addrs); // FULL mark"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn mark_sweep_if_over_watermark" "heap.mark(&addrs); // FULL mark" "(heap.sweep(), true)"
assert_after_before "src/backend/eval/cesk/index_heap.rs" "fn mark_sweep_if_over_watermark" "heap.mark_young(&addrs);" "(heap.sweep_young(), false)"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "fn ensure_eval_caches_gc_epoch_current()" "clear_eval_memo();" "clear_match_result_cache();"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "if stale {" "let evicted = memo.pop(&expr_hash);" "shade_evicted_eval_memo_entry(entry);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "if stale {" "with_satb_deletion_barrier" "let evicted = memo.pop(&expr_hash);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "if stale {" "if satb_active" "shade_evicted_eval_memo_entry(entry);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn eval_memo_put" "ensure_eval_caches_gc_epoch_current();" "let evicted = memo.push"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn eval_memo_put" "with_satb_deletion_barrier" "let evicted = memo.push"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn eval_memo_put" "if satb_active" "shade_evicted_eval_memo_entry(entry);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn eval_memo_put" "let evicted = memo.push" "shade_evicted_eval_memo_entry(entry);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn match_result_put" "ensure_eval_caches_gc_epoch_current();" "let evicted = cache.push"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn match_result_put" "with_satb_deletion_barrier" "let evicted = cache.push"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn match_result_put" "if satb_active" "shade_evicted_match_result_entry(evicted_entries);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn match_result_put" "let evicted = cache.push" "shade_evicted_match_result_entry(evicted_entries);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn collect_eval_memo_roots" "EVAL_MEMO.with" "out.extend_from_slice(entries);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn collect_match_result_roots" "MATCH_RESULT_CACHE.with" "out.push(*rhs);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn collect_match_result_roots" "out.push(*rhs);" "bindings.iter_full()"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn collect_match_result_roots" "bindings.iter_full()" "out.push(val.clone());"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn collect_match_result_roots" "if let Some(t) = rhs_type" "out.push(*t);"
assert_after_before "src/backend/bytecode/cache.rs" "pub fn cache_bytecode" "with_satb_deletion_barrier" "let mut cache = BYTECODE_CACHE.write();"
assert_after_before "src/backend/bytecode/cache.rs" "pub fn cache_bytecode" "let evicted = cache.push" "shade_evicted_bytecode_chunk(&evicted_chunk);"
assert_after_before "src/backend/bytecode/cache.rs" "pub fn cache_bytecode" "if satb_active" "shade_evicted_bytecode_chunk(&evicted_chunk);"
assert_after_before "src/backend/bytecode/cache.rs" "fn shade_evicted_bytecode_chunk" "collect_chunk_constants(chunk, &mut roots);" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_eval_memo()" "with_satb_deletion_barrier" "memo.clear();"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_eval_memo()" "if satb_active" "satb_shade_evicted_roots("
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_eval_memo()" "satb_shade_evicted_roots(" "memo.clear();"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_match_result_cache()" "with_satb_deletion_barrier" "cache.clear();"
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_match_result_cache()" "if satb_active" "satb_shade_evicted_roots("
assert_after_before "src/backend/eval/trampoline/dispatch_hints.rs" "pub fn clear_match_result_cache()" "satb_shade_evicted_roots(" "cache.clear();"
assert_after_before "src/backend/bytecode/cache.rs" "pub fn clear_caches()" "with_satb_deletion_barrier" "let mut bytecode_cache = BYTECODE_CACHE.write();"
assert_after_before "src/backend/bytecode/cache.rs" "pub fn clear_caches()" "if satb_active" "satb_shade_evicted_roots("
assert_after_before "src/backend/bytecode/cache.rs" "pub fn clear_caches()" "satb_shade_evicted_roots(" "bytecode_cache.clear();"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "global_tiered_cache().collect_roots_into(out);" "global_space_registry().collect_all_gc_values(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "global_memo_cache().collect_all_values(out);" "collect_bytecode_cache_roots(out);"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "fn shade_tiered_roots" "satb_shade_evicted_roots(roots);" "}"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "struct PendingBytecodeRootEntry" "token: u64" "root: MettaValue"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "struct PendingBytecodeRootGuard" "token: u64" "roots: Arc<DashMap"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "fn next_pending_bytecode_root_token" "fetch_update" "current.checked_add(1)"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "fn next_pending_bytecode_root_token" "current.checked_add(1)" "expect(\"pending bytecode root token counter exhausted\")"
line_no "src/backend/bytecode/tiered_cache.rs" "entry.token == self.token" >/dev/null
assert_after_before "src/backend/bytecode/tiered_cache.rs" "impl Drop for PendingBytecodeRootGuard" "with_satb_deletion_barrier" "remove_if(&self.expr_hash"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "impl Drop for PendingBytecodeRootGuard" "entry.token == self.token" "shade_tiered_roots(vec![root]);"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "fn register_pending_bytecode_root" "next_pending_bytecode_root_token()" "PendingBytecodeRootEntry { token, root: expr }"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "fn register_pending_bytecode_root" "with_satb_deletion_barrier" "pending_bytecode_roots.insert(expr_hash, entry);"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "fn register_pending_bytecode_root" "pending_bytecode_roots.insert(expr_hash, entry);" "shade_tiered_roots(vec![root.root]);"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "if !enqueued {" "with_satb_deletion_barrier" "remove_if(&state.expr_hash"
line_no "src/backend/bytecode/tiered_cache.rs" "entry.token == root_token" >/dev/null
assert_after_before "src/backend/bytecode/tiered_cache.rs" "if !enqueued {" "entry.token == root_token" "shade_tiered_roots(vec![root]);"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "pub fn clear(&self)" "with_satb_deletion_barrier" "self.entries.clear();"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "pub fn clear(&self)" "collect_chunk_constants(&chunk, &mut roots);" "shade_tiered_roots(roots);"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "pub fn clear(&self)" "shade_tiered_roots(roots);" "self.entries.clear();"
assert_after_before "src/backend/bytecode/tiered_cache.rs" "pub fn clear(&self)" "self.entries.clear();" "self.pending_bytecode_roots.clear();"
assert_after_before "src/backend/bytecode/memo_cache.rs" "fn shade_evicted_values" "downcast_ref::<MettaValue>()" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/bytecode/memo_cache.rs" "pub fn insert" "with_satb_deletion_barrier" "let mut evicted = Vec::new();"
assert_after_before "src/backend/bytecode/memo_cache.rs" "pub fn insert" "evicted.extend(self.evict_lru());" "shade_evicted_values(evicted);"
assert_after_before "src/backend/bytecode/memo_cache.rs" "pub fn insert" "evicted.push(old.result);" "shade_evicted_values(evicted);"
assert_after_before "src/backend/bytecode/memo_cache.rs" "pub fn clear" "with_satb_deletion_barrier" "self.cache.clear();"
assert_after_before "src/backend/bytecode/memo_cache.rs" "pub fn clear" "shade_evicted_values(roots);" "self.cache.clear();"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "global_tiered_cache().collect_roots_into(out);" "global_space_registry().collect_all_gc_values(out);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn shade_space_handle" "handle.collect_gc_values(&mut roots);" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn shade_all_spaces" "self.collect_all_gc_values(&mut roots);" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn register_with_satb" "with_satb_deletion_barrier" "self.spaces.insert(name.to_string(), handle);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn register_with_satb" "self.spaces.insert(name.to_string(), handle);" "Self::shade_space_handle(&old);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn remove_with_satb" "with_satb_deletion_barrier" "self.spaces.remove(name);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn remove_with_satb" "self.spaces.remove(name);" "Self::shade_space_handle(&old);"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn clear_with_satb" "with_satb_deletion_barrier" "self.spaces.clear();"
assert_after_before "src/backend/bytecode/space_registry.rs" "fn clear_with_satb" "self.shade_all_spaces();" "self.spaces.clear();"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "collect_bytecode_cache_roots(out);" "collect_compiler_atom_roots(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "collect_compiler_atom_roots(out);" "collect_eval_memo_roots(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "collect_eval_memo_roots(out);" "collect_match_result_roots(out);"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "collect_match_result_roots(out);" "collect_subgoal_roots(out);"
assert_count "src/backend/bytecode/compiler/iterative.rs" "OnceLock<MettaValue>" "3"
assert_zero "src/backend/bytecode/compiler/iterative.rs" ".take("
assert_zero "src/backend/bytecode/compiler/iterative.rs" ".set("
assert_after_before "src/backend/bytecode/compiler/iterative.rs" "pub(crate) fn collect_compiler_atom_roots" "ATOM_EQUALS.get()" "roots.push(*v);"
assert_after_before "src/backend/bytecode/compiler/iterative.rs" "if let Some(v) = ATOM_PRINTLN.get()" "roots.push(*v);" "if let Some(v) = ATOM_IF.get()"
assert_after_before "src/backend/bytecode/compiler/iterative.rs" "if let Some(v) = ATOM_IF.get()" "roots.push(*v);" "static COMPILER_ATOM_ROOT_PROVIDER"
assert_after_before "src/backend/eval/cesk/roots.rs" "pub fn collect_global_anchors" "collect_subgoal_roots(out);" "collect_thunk_roots(out);"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn shade_values" "downcast_ref::<MettaValue>()" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn insert_entry_with_satb" "with_satb_deletion_barrier" "self.entries.insert(expr_hash, entry);"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn insert_entry_with_satb" "self.entries.insert(expr_hash, entry);" "Self::shade_entry(old);"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn remove_entry_with_satb" "with_satb_deletion_barrier" "self.entries.remove(&expr_hash);"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn remove_entry_with_satb" "self.entries.remove(&expr_hash);" "Self::shade_entry(old);"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn clear_entries_with_satb" "with_satb_deletion_barrier" "self.entries.clear();"
assert_after_before "src/backend/eval/cesk/tabling.rs" "fn clear_entries_with_satb" "Self::shade_values(roots);" "self.entries.clear();"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn shade_values" "downcast_ref::<MettaValue>()" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn insert_thunk_with_satb" "with_satb_deletion_barrier" "self.entries.insert(expr_hash, thunk);"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn insert_thunk_with_satb" "self.entries.insert(expr_hash, thunk);" "Self::shade_thunk(old);"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn remove_thunk_with_satb" "with_satb_deletion_barrier" "self.entries.remove(&expr_hash);"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn remove_thunk_with_satb" "self.entries.remove(&expr_hash);" "Self::shade_thunk(old);"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn clear_entries_with_satb" "with_satb_deletion_barrier" "self.entries.clear();"
assert_after_before "src/backend/eval/cesk/thunk.rs" "fn clear_entries_with_satb" "Self::shade_values(roots);" "self.entries.clear();"
assert_after_before "src/backend/eval/cesk/thunk.rs" "pub fn update" "with_satb_deletion_barrier" "thunk.results = results;"
assert_after_before "src/backend/eval/cesk/thunk.rs" "pub fn update" "Self::shade_values(thunk.results.iter().cloned());" "thunk.results = results;"

# E2 SATB E0 mutation-site coupling: every value-bearing substore reached by
# `collect_roots_into` must shade the removed pre-image while holding the SATB
# phase gate. Byte-key PathMaps are excluded here because they do not store
# MettaValue handles; this block covers the nested containers that do.
assert_after_before "src/backend/environment/core.rs" "pub(crate) fn shade_generic_values_for_satb" "downcast_ref::<MettaValue>()" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/environment/core.rs" "pub(crate) fn with_env_satb_deletion_barrier" "with_satb_deletion_barrier(f)" "}"

assert_after_before "src/backend/environment/symbol_bindings.rs" "pub fn bind" "with_env_satb_deletion_barrier" ".insert(symbol.to_string(), value);"
assert_after_before "src/backend/environment/symbol_bindings.rs" "pub fn bind" ".insert(symbol.to_string(), value);" "shade_generic_values_for_satb(std::iter::once(old));"
assert_after_before "src/backend/environment/mutable_state.rs" "pub fn change_state" "with_env_satb_deletion_barrier" "std::mem::replace(entry, new_value.clone());"
assert_after_before "src/backend/environment/mutable_state.rs" "pub fn change_state" "std::mem::replace(entry, new_value.clone());" "shade_generic_values_for_satb(std::iter::once(old));"
assert_after_before "src/backend/environment/named_spaces.rs" "pub fn remove_from_named_space" "with_env_satb_deletion_barrier" "let removed = atoms.remove(pos);"
assert_after_before "src/backend/environment/named_spaces.rs" "pub fn remove_from_named_space" "let removed = atoms.remove(pos);" "shade_generic_values_for_satb(std::iter::once(removed));"
assert_after_before "src/backend/environment/core.rs" "let typ = &items[2];" "with_env_satb_deletion_barrier" "removed.push(vec.remove(idx));"
assert_after_before "src/backend/environment/core.rs" "let typ = &items[2];" "removed.push(vec.remove(idx));" "shade_generic_values_for_satb(removed);"
assert_after_before "src/backend/modules/tokenizer.rs" "fn shade_token_values" "downcast_ref::<MettaValue>()" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/modules/tokenizer.rs" "pub fn clear(&mut self)" "with_satb_deletion_barrier" "let roots = self.collect_gc_values();"
assert_after_before "src/backend/modules/tokenizer.rs" "pub fn clear(&mut self)" "shade_token_values(roots);" "self.tokens.clear();"
assert_after_before "src/backend/modules/tokenizer.rs" "pub fn remove_token" "with_satb_deletion_barrier" "self.tokens.retain(|entry|"
assert_after_before "src/backend/modules/tokenizer.rs" "pub fn remove_token" "removed_values.push" "shade_token_values(removed_values);"
assert_after_before "src/backend/environment/act_tiered.rs" "Clear the overlay" "with_env_satb_deletion_barrier" "variable_atoms.write().clear();"
assert_after_before "src/backend/environment/act_tiered.rs" "Clear the overlay" "shade_generic_values_for_satb(removed);" "variable_atoms.write().clear();"

assert_after_before "src/backend/models/space_handle.rs" "Variable atom → remove from Vec" "with_satb_deletion_barrier" "var_atoms.swap_remove(idx).0"
assert_after_before "src/backend/models/space_handle.rs" "Variable atom → remove from Vec" "var_atoms.swap_remove(idx).0" "satb_shade_evicted_roots("
assert_after_before "src/backend/modules/module_space.rs" "fn shade_module_atoms" "satb_shade_evicted_roots(atoms);" "}"
assert_after_before "src/backend/modules/module_space.rs" "pub fn remove_atom" "with_satb_deletion_barrier" "let removed = self.atoms.remove(pos);"
assert_after_before "src/backend/modules/module_space.rs" "pub fn remove_atom" "let removed = self.atoms.remove(pos);" "shade_module_atoms(std::iter::once(removed));"
assert_after_before "src/backend/modules/module_space.rs" "pub fn clear(&mut self)" "with_satb_deletion_barrier" "self.atoms.clear();"
assert_after_before "src/backend/modules/module_space.rs" "pub fn clear(&mut self)" "shade_module_atoms(self.atoms.iter().cloned());" "self.atoms.clear();"

assert_after_before "src/backend/environment/rule_management.rs" "fn shade_rule_entry_for_satb" "for value in [&entry.lhs, &entry.rhs]" "if let Some(rhs_type)"
assert_after_before "src/backend/environment/rule_management.rs" "fn shade_rule_entry_for_satb" "collect_chunk_constants(chunk, &mut roots);" "satb_shade_evicted_roots(roots);"
assert_after_before "src/backend/environment/rule_management.rs" "fn remove_rule(&mut self, lhs: &V, rhs: &V, satb_active: bool)" "let removed = entries.remove(pos);" "shade_rule_entry_for_satb(&removed);"
assert_after_before "src/backend/environment/rule_management.rs" "fn remove_rule_by_debruijn(" "let removed = entries.remove(pos);" "shade_rule_entry_for_satb(&removed);"
assert_after_before "src/backend/environment/rule_management.rs" "pub fn remove_rule(&mut self, lhs: &V, rhs: &V) -> bool" "with_satb_deletion_barrier" "self.remove_rule_inner(lhs, rhs, satb_active)"
assert_after_before "src/backend/environment/rule_management.rs" "fn remove_rule_inner" "group.remove_rule(lhs, rhs, satb_active)" "self.rule_rhs_atoms.note_rule_removed(h, rhs);"
assert_after_before "src/backend/environment/rule_management.rs" "fn remove_rule_inner" "let removed = self.wildcard.remove(pos);" "shade_rule_entry_for_satb(&removed);"
assert_after_before "src/backend/environment/rule_management.rs" "pub fn remove_rule_by_debruijn(&mut self, full_bytes: &[u8])" "with_satb_deletion_barrier" "self.remove_rule_by_debruijn_inner(full_bytes, satb_active)"
assert_after_before "src/backend/environment/rule_management.rs" "fn remove_rule_by_debruijn_inner" "group.remove_rule_by_debruijn(full_bytes, satb_active)" "self.rule_rhs_atoms.note_rule_removed(*head, &rhs);"
assert_after_before "src/backend/environment/rule_management.rs" "fn remove_rule_by_debruijn_inner" "let removed = self.wildcard.remove(pos);" "shade_rule_entry_for_satb(&removed);"
assert_after_before "src/backend/environment/rule_management.rs" "pub fn clear(&mut self)" "with_satb_deletion_barrier" "self.clear_inner(satb_active)"
assert_after_before "src/backend/environment/rule_management.rs" "fn clear_inner" "shade_rule_entry_for_satb(entry);" "self.by_head_arity.clear();"

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
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn close_open_rendezvous_cycle" "ga::end_rendezvous_cycle();" "drop(gip);"
assert_after_before "src/backend/eval/cesk/gc_driver.rs" "fn close_open_rendezvous_cycle" "drop(gip);" "ga::resume_workers();"

echo "CESK GC source-coupling checks passed"
