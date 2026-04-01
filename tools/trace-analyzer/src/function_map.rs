// function_map.rs — Category taxonomy bridging Rust function names and TraceEventKind variants.
//
// The key insight: perf/massif profiles name Rust functions, while .mtrace files record
// TraceEventKind variants. This module classifies both into a shared 17-category taxonomy,
// enabling cross-validation without timestamp correlation.

use trace_format::TraceEventKind;

/// Semantic category shared between native profiling (perf/massif) and MeTTa evaluation traces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceCategory {
    EvalCore,          // trampoline loop, step dispatch
    RuleMatching,      // try_match_all_rules, match_rules_native, RuleIndex
    PatternBinding,    // pattern_match, apply_bindings, unify, sealed
    GroundedOps,       // eval_grounded, find_grounded
    TypeSystem,        // get-type, check-type, infer, BranchPrune
    ControlFlow,       // if, let, chain, case, switch
    Nondeterminism,    // NondeterministicFork, branch start/end
    ListOps,           // car, cdr, cons, decons, size
    SetOps,            // union, intersection, subtraction, unique
    SpaceOps,          // match, collapse, add-atom, remove-atom
    Modules,           // import, include
    Allocation,        // alloc_value, alloc_str, alloc_slice, bump_alloc
    GarbageCollection, // gc_, safepoint, collect_roots
    BytecodeVM,        // bytecode, execute_generic
    JitCompilation,    // jit, native_fn
    MorkOps,           // exec, coalg, lookup, rulify, pathmap
    Other,
}

impl std::fmt::Display for TraceCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            TraceCategory::EvalCore => "EvalCore",
            TraceCategory::RuleMatching => "RuleMatching",
            TraceCategory::PatternBinding => "PatternBinding",
            TraceCategory::GroundedOps => "GroundedOps",
            TraceCategory::TypeSystem => "TypeSystem",
            TraceCategory::ControlFlow => "ControlFlow",
            TraceCategory::Nondeterminism => "Nondeterminism",
            TraceCategory::ListOps => "ListOps",
            TraceCategory::SetOps => "SetOps",
            TraceCategory::SpaceOps => "SpaceOps",
            TraceCategory::Modules => "Modules",
            TraceCategory::Allocation => "Allocation",
            TraceCategory::GarbageCollection => "GarbageCollection",
            TraceCategory::BytecodeVM => "BytecodeVM",
            TraceCategory::JitCompilation => "JitCompilation",
            TraceCategory::MorkOps => "MorkOps",
            TraceCategory::Other => "Other",
        };
        write!(f, "{}", name)
    }
}

/// All categories in iteration order (for table output).
pub const ALL_CATEGORIES: [TraceCategory; 17] = [
    TraceCategory::EvalCore,
    TraceCategory::RuleMatching,
    TraceCategory::PatternBinding,
    TraceCategory::GroundedOps,
    TraceCategory::TypeSystem,
    TraceCategory::ControlFlow,
    TraceCategory::Nondeterminism,
    TraceCategory::ListOps,
    TraceCategory::SetOps,
    TraceCategory::SpaceOps,
    TraceCategory::Modules,
    TraceCategory::Allocation,
    TraceCategory::GarbageCollection,
    TraceCategory::BytecodeVM,
    TraceCategory::JitCompilation,
    TraceCategory::MorkOps,
    TraceCategory::Other,
];

/// Strip Rust monomorphization hash suffix `::h[0-9a-f]{16}` and angle-bracket generics.
fn normalize_rust_name(name: &str) -> String {
    let mut s = name.to_string();

    // Strip ::h<hex16> hash suffix
    if let Some(idx) = s.rfind("::h") {
        let suffix = &s[idx + 3..];
        if suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
            s.truncate(idx);
        }
    }

    // Strip <...> generics (outermost only, to preserve readability)
    while let Some(start) = s.find('<') {
        if let Some(end) = s.rfind('>') {
            if end > start {
                s = format!("{}{}", &s[..start], &s[end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    s
}

/// Extract the leaf function name from a qualified path.
/// e.g., `mettatron::backend::eval::trampoline::eval_trampoline_generic` → `eval_trampoline_generic`
pub fn extract_leaf_name(qualified: &str) -> &str {
    qualified
        .rsplit("::")
        .next()
        .unwrap_or(qualified)
}

/// Classify a (demangled) Rust function name into a TraceCategory.
///
/// Names are normalized (hash suffix + generics stripped) then matched by substring,
/// most-specific first.
pub fn classify_rust_function(name: &str) -> TraceCategory {
    let norm = normalize_rust_name(name);
    let leaf = extract_leaf_name(&norm);

    // Most-specific matches first

    // GC
    if norm.contains("gc_") || norm.contains("safepoint") || norm.contains("collect_roots")
        || norm.contains("mark_sweep") || norm.contains("GcThread")
        || leaf.starts_with("gc_") || norm.contains("quiescent")
    {
        return TraceCategory::GarbageCollection;
    }

    // Allocation
    if norm.contains("alloc_value") || norm.contains("alloc_str") || norm.contains("alloc_slice")
        || norm.contains("bump_alloc") || norm.contains("SlabAllocator")
        || norm.contains("slab_alloc") || leaf == "allocate"
    {
        return TraceCategory::Allocation;
    }

    // JIT
    if norm.contains("jit") || norm.contains("native_fn") || norm.contains("JitCompiler")
        || norm.contains("compile_jit") || norm.contains("jit_to_value")
        || norm.contains("value_to_jit")
    {
        return TraceCategory::JitCompilation;
    }

    // Bytecode VM
    if norm.contains("bytecode") || norm.contains("execute_generic")
        || norm.contains("BytecodeVM") || norm.contains("compile_bytecode")
        || norm.contains("vm_execute")
    {
        return TraceCategory::BytecodeVM;
    }

    // MORK
    if norm.contains("mork") || norm.contains("coalg") || norm.contains("rulify")
        || norm.contains("pathmap") || norm.contains("PathMap")
        || norm.contains("SharedMapping") || norm.contains("trie_query")
    {
        return TraceCategory::MorkOps;
    }

    // Modules
    if norm.contains("import") || norm.contains("include") || norm.contains("module") {
        return TraceCategory::Modules;
    }

    // Space ops
    if norm.contains("match_space") || norm.contains("collapse_space")
        || norm.contains("add_atom") || norm.contains("remove_atom")
        || norm.contains("get_atoms") || norm.contains("SpaceHandle")
    {
        return TraceCategory::SpaceOps;
    }

    // Set ops
    if norm.contains("eval_union") || norm.contains("eval_intersection")
        || norm.contains("eval_subtraction") || norm.contains("eval_unique")
        || (leaf.starts_with("eval_") && (leaf.contains("union") || leaf.contains("intersection")
            || leaf.contains("subtraction") || leaf.contains("unique")))
    {
        return TraceCategory::SetOps;
    }

    // List ops
    if norm.contains("eval_car") || norm.contains("eval_cdr") || norm.contains("eval_cons")
        || norm.contains("eval_decons") || norm.contains("eval_size")
        || norm.contains("list_ops") || norm.contains("eval_tuple")
    {
        return TraceCategory::ListOps;
    }

    // Control flow
    if norm.contains("eval_if") || norm.contains("eval_let") || norm.contains("eval_chain")
        || norm.contains("eval_case") || norm.contains("eval_switch")
        || norm.contains("control_flow") || norm.contains("ProcessIf")
        || norm.contains("ProcessLet") || norm.contains("ProcessChain")
    {
        return TraceCategory::ControlFlow;
    }

    // Nondeterminism
    if norm.contains("nondeterministic") || norm.contains("NondeterministicFork")
        || norm.contains("BranchStart") || norm.contains("BranchEnd")
        || norm.contains("superpose") || norm.contains("dispatch_rule_matches")
        || norm.contains("parallel_branch")
    {
        return TraceCategory::Nondeterminism;
    }

    // Type system
    if norm.contains("type_") || norm.contains("infer_type") || norm.contains("check_type")
        || norm.contains("get_type") || norm.contains("TypeSignature")
        || norm.contains("BranchPrune") || norm.contains("TypeBloomFilter")
        || norm.contains("TypeRegistry")
    {
        return TraceCategory::TypeSystem;
    }

    // Grounded ops
    if norm.contains("eval_grounded") || norm.contains("find_grounded")
        || norm.contains("is_grounded_op") || norm.contains("grounded_registry")
    {
        return TraceCategory::GroundedOps;
    }

    // Pattern binding
    if norm.contains("pattern_match") || norm.contains("apply_bindings")
        || norm.contains("unify") || norm.contains("sealed_")
        || norm.contains("try_bind") || norm.contains("bindings")
        || norm.contains("substitut")
    {
        return TraceCategory::PatternBinding;
    }

    // Rule matching
    if norm.contains("match_rules") || norm.contains("try_match_all")
        || norm.contains("RuleIndex") || norm.contains("rule_match")
        || norm.contains("rule_index") || norm.contains("match_all_rules")
    {
        return TraceCategory::RuleMatching;
    }

    // Eval core (broad — must be last among eval-related categories)
    if norm.contains("eval_trampoline") || norm.contains("eval_sexpr_step")
        || norm.contains("eval_inner") || norm.contains("trampoline")
        || norm.contains("EvalGuard") || norm.contains("step_dispatch")
        || leaf == "eval" || leaf.starts_with("eval_") && !leaf.contains("_grounded")
    {
        return TraceCategory::EvalCore;
    }

    TraceCategory::Other
}

/// Classify a TraceEventKind variant into a TraceCategory.
pub fn classify_trace_event(kind: &TraceEventKind) -> TraceCategory {
    match kind {
        TraceEventKind::RuleApplication { .. } | TraceEventKind::RuleMatchSet { .. } => {
            TraceCategory::RuleMatching
        }

        TraceEventKind::PatternMatch { .. } => TraceCategory::PatternBinding,

        TraceEventKind::GroundedOp { op_name, .. } => {
            // Sub-classify grounded ops
            let op = op_name.as_str();
            match op {
                "car-atom" | "cdr-atom" | "cons-atom" | "decons-atom" | "size-atom"
                | "tuple-count" | "tuple-concat" => TraceCategory::ListOps,
                "union" | "intersection" | "subtraction" | "unique" => TraceCategory::SetOps,
                "collapse" | "get-atoms" => TraceCategory::SpaceOps,
                "add-atom" | "remove-atom" => TraceCategory::SpaceOps,
                "get-type" | "check-type" => TraceCategory::TypeSystem,
                _ => TraceCategory::GroundedOps,
            }
        }

        TraceEventKind::SpecialForm { form_name, .. } => {
            let form = form_name.as_str();
            match form {
                "if" | "let" | "let*" | "chain" | "case" | "switch" | "do" | "sequential" => {
                    TraceCategory::ControlFlow
                }
                "match" | "collapse" => TraceCategory::SpaceOps,
                "import!" | "include" | "import" | "bind!" | "pragma!" => TraceCategory::Modules,
                "superpose" => TraceCategory::Nondeterminism,
                _ => TraceCategory::ControlFlow,
            }
        }

        TraceEventKind::NondeterministicFork { .. }
        | TraceEventKind::BranchStart { .. }
        | TraceEventKind::BranchEnd { .. } => TraceCategory::Nondeterminism,

        TraceEventKind::TypeOperation { .. }
        | TraceEventKind::TypeInference { .. }
        | TraceEventKind::TypeMatch { .. }
        | TraceEventKind::BranchPrune { .. }
        | TraceEventKind::InferredTypeRegistered { .. }
        | TraceEventKind::RhsTypeComputed { .. } => TraceCategory::TypeSystem,

        TraceEventKind::ApplicativePreEval { .. } => TraceCategory::TypeSystem,

        TraceEventKind::GcSafepoint { .. } => TraceCategory::GarbageCollection,

        TraceEventKind::TierDispatch { .. }
        | TraceEventKind::EvalStart
        | TraceEventKind::EvalEnd { .. } => TraceCategory::EvalCore,

        TraceEventKind::BytecodeCompilation { .. } | TraceEventKind::BytecodeHalt { .. } => {
            TraceCategory::BytecodeVM
        }

        TraceEventKind::JitCompilation { .. } | TraceEventKind::JitBailout { .. } => {
            TraceCategory::JitCompilation
        }

        TraceEventKind::ErrorCreated { .. }
        | TraceEventKind::ErrorCaught { .. }
        | TraceEventKind::ErrorPropagated { .. }
        | TraceEventKind::GroundedOpError { .. } => TraceCategory::EvalCore,

        // WorkPool events
        TraceEventKind::WorkPoolTaskEnqueued { .. }
        | TraceEventKind::WorkPoolTaskDropped { .. }
        | TraceEventKind::WorkPoolTaskCompleted { .. }
        | TraceEventKind::WorkPoolScaleEvent { .. }
        | TraceEventKind::WorkPoolWorkerParked { .. }
        | TraceEventKind::WorkPoolWorkerResumed { .. }
        | TraceEventKind::WorkPoolBlockedWorkersDetected { .. }
        | TraceEventKind::WorkPoolCompensatoryAction { .. }
        | TraceEventKind::WorkPoolMonitorTick { .. }
        | TraceEventKind::WorkPoolWorkerBlocked { .. }
        | TraceEventKind::WorkPoolWorkerUnblocked { .. }
        | TraceEventKind::LetBindingStep { .. }
        | TraceEventKind::ArgumentPreEvalResult { .. }
        | TraceEventKind::TablingDecision { .. }
        | TraceEventKind::BindingsApplied { .. }
        | TraceEventKind::RuleSelected { .. } => TraceCategory::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_leaf_name() {
        assert_eq!(
            extract_leaf_name("mettatron::backend::eval::trampoline::eval_trampoline_generic"),
            "eval_trampoline_generic"
        );
        assert_eq!(extract_leaf_name("eval_grounded"), "eval_grounded");
        assert_eq!(extract_leaf_name(""), "");
    }

    #[test]
    fn test_normalize_rust_name() {
        assert_eq!(
            normalize_rust_name("eval_inner::h1234567890abcdef"),
            "eval_inner"
        );
        assert_eq!(
            normalize_rust_name("HashMap<String, Vec<u64>>::insert"),
            "HashMap::insert"
        );
        // Short hex suffix should NOT be stripped
        assert_eq!(normalize_rust_name("eval_inner::habcd"), "eval_inner::habcd");
    }

    #[test]
    fn test_classify_rust_function_eval_core() {
        assert_eq!(
            classify_rust_function("mettatron::backend::eval::trampoline::eval_trampoline_generic"),
            TraceCategory::EvalCore
        );
        assert_eq!(
            classify_rust_function("eval_sexpr_step_generic::h0123456789abcdef"),
            TraceCategory::EvalCore
        );
    }

    #[test]
    fn test_classify_rust_function_rule_matching() {
        assert_eq!(
            classify_rust_function("match_rules_native"),
            TraceCategory::RuleMatching
        );
        assert_eq!(
            classify_rust_function("RuleIndex::try_match_all"),
            TraceCategory::RuleMatching
        );
    }

    #[test]
    fn test_classify_rust_function_pattern_binding() {
        assert_eq!(
            classify_rust_function("pattern_match_recursive"),
            TraceCategory::PatternBinding
        );
        assert_eq!(
            classify_rust_function("apply_bindings_generic"),
            TraceCategory::PatternBinding
        );
    }

    #[test]
    fn test_classify_rust_function_grounded() {
        assert_eq!(
            classify_rust_function("eval_grounded_arithmetic"),
            TraceCategory::GroundedOps
        );
    }

    #[test]
    fn test_classify_rust_function_gc() {
        assert_eq!(
            classify_rust_function("gc_mark_sweep"),
            TraceCategory::GarbageCollection
        );
        assert_eq!(
            classify_rust_function("safepoint_wait_for_quiescence"),
            TraceCategory::GarbageCollection
        );
    }

    #[test]
    fn test_classify_rust_function_allocation() {
        assert_eq!(
            classify_rust_function("SlabAllocator::alloc_value"),
            TraceCategory::Allocation
        );
    }

    #[test]
    fn test_classify_rust_function_jit() {
        assert_eq!(
            classify_rust_function("jit_compile_expression"),
            TraceCategory::JitCompilation
        );
    }

    #[test]
    fn test_classify_rust_function_bytecode() {
        assert_eq!(
            classify_rust_function("BytecodeVM::execute_generic"),
            TraceCategory::BytecodeVM
        );
    }

    #[test]
    fn test_classify_rust_function_mork() {
        assert_eq!(
            classify_rust_function("PathMap::coalg_step"),
            TraceCategory::MorkOps
        );
    }

    #[test]
    fn test_classify_rust_function_other() {
        assert_eq!(
            classify_rust_function("std::io::read_to_string"),
            TraceCategory::Other
        );
    }

    #[test]
    fn test_classify_trace_event_rule_application() {
        let kind = TraceEventKind::RuleApplication {
            rule_lhs: trace_format::TraceValue::Atom("x".into()),
            rule_rhs: trace_format::TraceValue::Atom("y".into()),
            bindings: vec![],
            rule_span: None,
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::RuleMatching);
    }

    #[test]
    fn test_classify_trace_event_grounded_list_op() {
        let kind = TraceEventKind::GroundedOp {
            op_name: "car-atom".into(),
            args: vec![],
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::ListOps);
    }

    #[test]
    fn test_classify_trace_event_grounded_set_op() {
        let kind = TraceEventKind::GroundedOp {
            op_name: "union".into(),
            args: vec![],
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::SetOps);
    }

    #[test]
    fn test_classify_trace_event_special_form_if() {
        let kind = TraceEventKind::SpecialForm {
            form_name: "if".into(),
            phase: "cond".into(),
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::ControlFlow);
    }

    #[test]
    fn test_classify_trace_event_special_form_match() {
        let kind = TraceEventKind::SpecialForm {
            form_name: "match".into(),
            phase: "eval".into(),
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::SpaceOps);
    }

    #[test]
    fn test_classify_trace_event_special_form_import() {
        let kind = TraceEventKind::SpecialForm {
            form_name: "import!".into(),
            phase: "load".into(),
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::Modules);
    }

    #[test]
    fn test_classify_trace_event_nondeterministic_fork() {
        let kind = TraceEventKind::NondeterministicFork { branch_count: 3 };
        assert_eq!(classify_trace_event(&kind), TraceCategory::Nondeterminism);
    }

    #[test]
    fn test_classify_trace_event_type_system() {
        let kind = TraceEventKind::TypeInference {
            expression: trace_format::TraceValue::Atom("x".into()),
            inferred_types: vec![],
            source: "test".into(),
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::TypeSystem);
    }

    #[test]
    fn test_classify_trace_event_gc() {
        let kind = TraceEventKind::GcSafepoint {
            root_count: 10,
            allocation_delta_bytes: 1024,
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::GarbageCollection);
    }

    #[test]
    fn test_classify_trace_event_eval_start() {
        let kind = TraceEventKind::EvalStart;
        assert_eq!(classify_trace_event(&kind), TraceCategory::EvalCore);
    }

    #[test]
    fn test_classify_trace_event_workpool() {
        let kind = TraceEventKind::WorkPoolWorkerParked {
            worker_id: 0,
            queue_depth: 5,
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::Other);
    }

    #[test]
    fn test_classify_trace_event_superpose() {
        let kind = TraceEventKind::SpecialForm {
            form_name: "superpose".into(),
            phase: "eval".into(),
        };
        assert_eq!(classify_trace_event(&kind), TraceCategory::Nondeterminism);
    }
}
