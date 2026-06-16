//! A4.2b — Typed K-spine thread-locals (the structural representation of the
//! part of the CESK continuation register **K** that lives on the native Rust
//! stack across a NESTED `eval_trampoline`).
//!
//! Two thread-locals replace `frame_chain`'s type-erased raw-pointer chain with
//! a *typed*, structurally-walkable representation:
//!
//! - [`SUSPENDED_ACTIVATIONS`] — the spine of suspended OUTER trampoline
//!   activations: each holds either its (C, K) registers (`current_work` +
//!   `work_stack` + `continuations`) or a caller-held `Vec` of compiled import/assert
//!   expressions, live across the nested call.
//! - [`LIVE_VM_STACK`] — live bytecode-VM leaves: each holds the VM object
//!   (decoded via `GenericBytecodeVM::collect_roots_into`) or a VM template's
//!   frozen saved-bindings `Vec`.
//!
//! ## Discipline (mirrors `frame_chain`, now typed)
//!
//! Records hold **raw pointers to live data**, read at collection time — they
//! NEVER clone-at-push. `work_stack` mutates every reduction; a snapshot taken
//! at push time would go stale before the safepoint and miss freshly-pushed
//! roots (breaking the A4.3 oracle's `OLD ⊆ NEW` and risking use-after-free).
//! Each pointer names the SAME live datum its sibling `EvalFrameGuard` already
//! pins, with LIFO drop order and data-outlives-guard — so there is no UAF mode
//! beyond `frame_chain`'s (already sound).
//!
//! ## Additive / byte-identical (A4.2b)
//!
//! The call sites push these records at the K-spine sites, gated on
//! `gc_mode_is_index()` (historically ALONGSIDE the `frame_chain` guards that
//! A5/F4 later deleted). The thread-locals are *write-only* in the hot path;
//! they are read only by [`collect_k_spine`] (tests + the A4.3 machine-
//! equivalence oracle + the A4.4 safepoint).

use std::cell::RefCell;

use crate::backend::bytecode::vm::GenericBytecodeVM;
use crate::backend::models::{ActiveFactory, MettaValue};

use super::super::trampoline::{Continuation, WorkItem};

/// One suspended OUTER trampoline activation parked across a NESTED eval. Its
/// registers are held by raw pointer and read live (never cloned at push).
pub(crate) enum SuspendedActivation {
    /// Site #1 (`eval_trampoline_inner`): the (C, K) projection of an
    /// activation — its in-flight `current_work` plus pending `work_stack` (C)
    /// and `continuations` (K). The current-work slot closes the native-stack
    /// hole where an outer trampoline has popped a work item and then enters a
    /// nested evaluator before pushing its successor work.
    Spine {
        current_work: *const Option<WorkItem>,
        work_stack: *const Vec<WorkItem>,
        continuations: *const Vec<Continuation>,
    },
    /// Module-import / assertion sites: a caller-held `Vec<MettaValue>` of
    /// compiled expressions live across the nested eval but not (yet) in any
    /// C/K — the index analogue of the deleted `frame_chain::collect_vec_roots`.
    ExprVec { exprs: *const Vec<MettaValue> },
}

/// One live bytecode-VM (or JIT) leaf parked across a nested eval.
pub(crate) enum VmLeaf {
    /// Sites #7/#8/#9 (`with_vm_roots_frame`): the whole VM, decoded via
    /// `GenericBytecodeVM::collect_roots_into`. Pins the only nested-eval-
    /// reaching monomorphization `<MettaValue, ActiveFactory>` (the same
    /// assumption `with_vm_roots_frame` already makes).
    Vm {
        vm: *const GenericBytecodeVM<MettaValue, ActiveFactory>,
    },
    /// Sites #10/#11: a VM template's saved-bindings `Vec`, built once and never
    /// mutated (a frozen snapshot — so holding its pointer is staleness-safe).
    SavedBindings { bindings: *const Vec<MettaValue> },
    /// A VM-owned native-stack vector of transient local values live across a
    /// nested trampoline call but not stored in the VM struct itself. Used for
    /// type-driven pre-eval locals such as `expr`, `item_to_eval`, and
    /// `per_arg_results`.
    #[allow(dead_code)] // Constructed by index-gc VM rooting; slab check has no producer.
    ValueVec { values: *const Vec<MettaValue> },
    /// B4: a live JIT execution's `JitContext` — the JIT analogue of `Vm`. Its
    /// operand stack / results / choice-points / binding-frames / saved-stack /
    /// template-results carry arena `Addr`s, walked structurally via
    /// `collect_jit_roots_into`. The pointer is read LIVE at the nested-eval
    /// safepoint (the buffers mutate during JIT execution, so a snapshot would
    /// miss freshly-pushed roots — the same discipline as `Vm`). Pushed around
    /// `native_fn` only under index-gc (slab keeps its own worker-safepoint path).
    Jit {
        ctx: *const crate::backend::bytecode::jit::types::JitContext,
    },
}

thread_local! {
    /// Stack of suspended trampoline activations (push on guard ctor, pop on
    /// Drop; LIFO, in lock-step with the sibling `frame_chain` guards).
    static SUSPENDED_ACTIVATIONS: RefCell<Vec<SuspendedActivation>> =
        const { RefCell::new(Vec::new()) };
    /// Stack of live bytecode-VM leaves (same discipline).
    static LIVE_VM_STACK: RefCell<Vec<VmLeaf>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard for one [`SuspendedActivation`] record. Pushes on construction,
/// pops on `Drop`.
pub(crate) struct SuspendedActivationGuard;

impl SuspendedActivationGuard {
    /// Push a suspended-activation record onto the K-spine.
    ///
    /// # Safety
    /// The raw pointers inside `record` must point to data that outlives the
    /// returned guard (the caller pins them in the same scope, exactly as for
    /// the sibling `frame_chain::EvalFrameGuard`). LIFO drop order is required.
    #[inline]
    pub(crate) unsafe fn push(record: SuspendedActivation) -> Self {
        SUSPENDED_ACTIVATIONS.with(|s| s.borrow_mut().push(record));
        SuspendedActivationGuard
    }
}

impl Drop for SuspendedActivationGuard {
    #[inline]
    fn drop(&mut self) {
        SUSPENDED_ACTIVATIONS.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// RAII guard for one [`VmLeaf`] record. Pushes on construction, pops on `Drop`.
pub(crate) struct VmLeafGuard;

impl VmLeafGuard {
    /// Push a VM-leaf record onto the K-spine.
    ///
    /// # Safety
    /// The raw pointer inside `record` must point to a VM (or `Vec`) that
    /// outlives the returned guard, and (for `Vm`) the VM's concrete type must
    /// be `GenericBytecodeVM<MettaValue, ActiveFactory>`. LIFO drop order.
    #[inline]
    pub(crate) unsafe fn push(record: VmLeaf) -> Self {
        LIVE_VM_STACK.with(|s| s.borrow_mut().push(record));
        VmLeafGuard
    }
}

impl Drop for VmLeafGuard {
    #[inline]
    fn drop(&mut self) {
        LIVE_VM_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Read the typed native-stack K-spine **structurally**, appending its live
/// roots to `out`. Walks [`SUSPENDED_ACTIVATIONS`] (each `Spine` → reach its C
/// and K via `WorkItem`/`Continuation::collect_values`; each `ExprVec` →
/// `extend_from_slice`) and [`LIVE_VM_STACK`] (each `Vm` →
/// `collect_roots_into`; each `SavedBindings` → `extend_from_slice`). Reuses the
/// EXACT decoders `frame_chain` uses, so the result is term-by-term identical to
/// `collect_frame_chain_roots` over the migrated sites. Appends (never clears).
///
/// Additive / unused in the hot path until A4.4 (only tests + the A4.3 oracle +
/// the A4.4 safepoint call it).
pub fn collect_k_spine(out: &mut Vec<MettaValue>) {
    SUSPENDED_ACTIVATIONS.with(|s| {
        for act in s.borrow().iter() {
            match *act {
                // SAFETY: pointers valid by the guard-lifetime invariant — the
                // pushing activation has not returned, so its Vecs are in scope.
                SuspendedActivation::Spine {
                    current_work,
                    work_stack,
                    continuations,
                } => unsafe {
                    if let Some(w) = (*current_work).as_ref() {
                        w.collect_values(out);
                    }
                    for w in (*work_stack).iter() {
                        w.collect_values(out);
                    }
                    for c in (*continuations).iter() {
                        c.collect_values(out);
                    }
                },
                SuspendedActivation::ExprVec { exprs } => unsafe {
                    out.extend_from_slice(&*exprs);
                },
            }
        }
    });
    LIVE_VM_STACK.with(|s| {
        for leaf in s.borrow().iter() {
            match *leaf {
                // SAFETY: as above; the VM / Vec outlives the guard.
                VmLeaf::Vm { vm } => unsafe {
                    (*vm).collect_roots_into(out);
                },
                VmLeaf::SavedBindings { bindings } => unsafe {
                    out.extend_from_slice(&*bindings);
                },
                VmLeaf::ValueVec { values } => unsafe {
                    out.extend_from_slice(&*values);
                },
                // SAFETY: the JitContext outlives the guard (the HybridExecutor
                // holds its buffers across `native_fn`); read live at the
                // safepoint. `collect_jit_roots_into` walks every value-bearing
                // JIT field, decoding each Addr-bearing JitValue via the
                // index-aware `collect_jit_value_into` (B4.1).
                VmLeaf::Jit { ctx } => unsafe {
                    crate::backend::bytecode::jit::runtime::gc_roots::collect_jit_roots_into(
                        &*ctx, out,
                    );
                },
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::vm::BytecodeVM;
    use crate::backend::bytecode::ChunkBuilder;
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::MettaValueFactory;
    use std::sync::Arc;

    fn sorted_ptrs(v: &[MettaValue]) -> Vec<usize> {
        let mut p: Vec<usize> = v.iter().map(|x| x.inner_ptr() as usize).collect();
        p.sort_unstable();
        p
    }

    /// Build a one-item `work_stack` (an `Eval` of `value`) for Spine tests.
    fn work_stack_with(value: MettaValue) -> Vec<WorkItem> {
        use crate::backend::environment::MettaEnvironment;
        vec![WorkItem::Eval {
            value,
            env: Arc::new(MettaEnvironment::default()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
        }]
    }

    /// LOAD-BEARING: the `Spine` arm reads the full suspended C/K projection:
    /// the in-flight current work item, the pending `work_stack`, and the
    /// continuation stack.
    #[test]
    fn test_kspine_spine_reads_current_work_stack_and_kont() {
        let f = global_factory();
        let current_work = Some(WorkItem::Eval {
            value: f.atom("current-work-root"),
            env: Arc::new(crate::backend::environment::MettaEnvironment::default()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
        });
        let work_stack = work_stack_with(f.long(7));
        let continuations = vec![Continuation::Done];

        // Expected = collect_values over every work item + continuation.
        let mut expected: Vec<MettaValue> = Vec::new();
        if let Some(w) = current_work.as_ref() {
            w.collect_values(&mut expected);
        }
        for w in work_stack.iter() {
            w.collect_values(&mut expected);
        }
        for c in continuations.iter() {
            c.collect_values(&mut expected);
        }

        let mut got: Vec<MettaValue> = Vec::new();
        {
            // SAFETY: work_stack/continuations outlive the guard (same scope).
            let _g = unsafe {
                SuspendedActivationGuard::push(SuspendedActivation::Spine {
                    current_work: &current_work as *const Option<WorkItem>,
                    work_stack: &work_stack as *const Vec<WorkItem>,
                    continuations: &continuations as *const Vec<Continuation>,
                })
            };
            collect_k_spine(&mut got);
        }
        assert_eq!(sorted_ptrs(&got), sorted_ptrs(&expected));
        assert!(got.contains(&f.long(7)) || sorted_ptrs(&got) == sorted_ptrs(&expected));

        // Guard dropped → empty.
        let mut after = Vec::new();
        collect_k_spine(&mut after);
        assert!(after.is_empty());
    }

    /// The `ExprVec` arm == `extend_from_slice` (= frame_chain's collect_vec_roots).
    #[test]
    fn test_kspine_exprvec_equals_extend() {
        let f = global_factory();
        let exprs = vec![f.atom("a"), f.long(42), f.bool(true)];
        let mut got = Vec::new();
        {
            // SAFETY: exprs outlives the guard.
            let _g = unsafe {
                SuspendedActivationGuard::push(SuspendedActivation::ExprVec {
                    exprs: &exprs as *const Vec<MettaValue>,
                })
            };
            collect_k_spine(&mut got);
        }
        assert_eq!(sorted_ptrs(&got), sorted_ptrs(&exprs));
    }

    /// The `SavedBindings` arm == `extend_from_slice`.
    #[test]
    fn test_kspine_savedbindings_equals_extend() {
        let f = global_factory();
        let bindings = vec![f.atom("x"), f.long(9)];
        let mut got = Vec::new();
        {
            // SAFETY: bindings outlives the guard.
            let _g = unsafe {
                VmLeafGuard::push(VmLeaf::SavedBindings {
                    bindings: &bindings as *const Vec<MettaValue>,
                })
            };
            collect_k_spine(&mut got);
        }
        assert_eq!(sorted_ptrs(&got), sorted_ptrs(&bindings));
    }

    /// The `ValueVec` arm roots VM-owned native-stack locals that are not stored
    /// in the VM struct itself.
    #[test]
    fn test_kspine_valuevec_equals_extend() {
        let f = global_factory();
        let values = vec![f.atom("vm-local"), f.long(17)];
        let mut got = Vec::new();
        {
            // SAFETY: values outlives the guard.
            let _g = unsafe {
                VmLeafGuard::push(VmLeaf::ValueVec {
                    values: &values as *const Vec<MettaValue>,
                })
            };
            collect_k_spine(&mut got);
        }
        assert_eq!(sorted_ptrs(&got), sorted_ptrs(&values));
    }

    /// The `Vm` arm == `vm.collect_roots_into` (delegates to the same method
    /// vm/tests.rs exercises). Non-vacuous: the chunk carries a constant.
    #[test]
    fn test_kspine_vm_equals_collect_roots_into() {
        let root = MettaValue::sym("kspine-vm-root");
        let chunk = {
            let mut b = ChunkBuilder::new("kspine-vm");
            b.add_constant(root);
            b.build_arc()
        };
        let vm = BytecodeVM::new(Arc::clone(&chunk));

        let mut want = Vec::new();
        vm.collect_roots_into(&mut want);

        let mut got = Vec::new();
        {
            // SAFETY: vm outlives the guard; its type is the pinned monomorph.
            let _g = unsafe {
                VmLeafGuard::push(VmLeaf::Vm {
                    vm: &vm as *const GenericBytecodeVM<MettaValue, ActiveFactory>,
                })
            };
            collect_k_spine(&mut got);
        }
        assert_eq!(sorted_ptrs(&got), sorted_ptrs(&want));
        assert!(want.contains(&root), "VM chunk constant must be a root");
    }

    /// Nested push/pop is LIFO: both records visible while both guards live;
    /// only the outer remains after the inner drops.
    #[test]
    fn test_kspine_nested_lifo() {
        let f = global_factory();
        let outer = vec![f.atom("outer1"), f.atom("outer2")];
        let inner = vec![f.atom("inner1")];

        let mut both = Vec::new();
        {
            let _o = unsafe {
                SuspendedActivationGuard::push(SuspendedActivation::ExprVec {
                    exprs: &outer as *const Vec<MettaValue>,
                })
            };
            {
                let _i = unsafe {
                    SuspendedActivationGuard::push(SuspendedActivation::ExprVec {
                        exprs: &inner as *const Vec<MettaValue>,
                    })
                };
                collect_k_spine(&mut both);
            }
            // Inner dropped → only outer remains.
            let mut after_inner = Vec::new();
            collect_k_spine(&mut after_inner);
            assert_eq!(after_inner.len(), 2);
        }
        assert_eq!(both.len(), 3);
        let mut after_all = Vec::new();
        collect_k_spine(&mut after_all);
        assert!(after_all.is_empty());
    }

    /// REFERENCE, not snapshot: a record reads the CURRENT contents of the
    /// pointed-to Vec, so a mutation AFTER the push is visible. A clone-at-push
    /// implementation would fail this (and break the A4.3 oracle / risk UAF).
    #[test]
    fn test_kspine_reference_not_snapshot() {
        let f = global_factory();
        let current_work: Option<WorkItem> = None;
        let mut work_stack: Vec<WorkItem> = Vec::new();
        let continuations: Vec<Continuation> = vec![Continuation::Done];
        let v = f.long(0xBEEF);

        // SAFETY: both Vecs outlive the guard (declared above it).
        let _g = unsafe {
            SuspendedActivationGuard::push(SuspendedActivation::Spine {
                current_work: &current_work as *const Option<WorkItem>,
                work_stack: &work_stack as *const Vec<WorkItem>,
                continuations: &continuations as *const Vec<Continuation>,
            })
        };
        // Push an item AFTER the guard already references the (then-empty) Vec.
        work_stack.extend(work_stack_with(v));

        let mut got = Vec::new();
        collect_k_spine(&mut got);
        assert!(
            got.iter().any(|x| x.inner_ptr() == v.inner_ptr()),
            "collect_k_spine must read the Vec's CURRENT contents (reference, not snapshot)"
        );
        drop(_g);
    }
}
