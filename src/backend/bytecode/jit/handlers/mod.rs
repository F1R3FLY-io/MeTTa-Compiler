//! Opcode handlers for JIT compilation
//!
//! This module contains handlers for each category of bytecode opcodes.
//! Each handler module compiles specific opcodes to Cranelift IR.

#[cfg(feature = "jit")]
mod stack;
#[cfg(feature = "jit")]
mod values;
#[cfg(feature = "jit")]
mod arithmetic;
#[cfg(feature = "jit")]
mod comparison;
#[cfg(feature = "jit")]
mod locals;
#[cfg(feature = "jit")]
mod type_predicates;
#[cfg(feature = "jit")]
mod math;
#[cfg(feature = "jit")]
mod sexpr;
#[cfg(feature = "jit")]
mod expr;
#[cfg(feature = "jit")]
mod control_flow;
#[cfg(feature = "jit")]
mod calls;
#[cfg(feature = "jit")]
mod nondet;
#[cfg(feature = "jit")]
mod bindings;
#[cfg(feature = "jit")]
mod pattern_matching;
#[cfg(feature = "jit")]
mod space;
#[cfg(feature = "jit")]
mod rules;
#[cfg(feature = "jit")]
mod special_forms;
#[cfg(feature = "jit")]
mod mork;
#[cfg(feature = "jit")]
mod debug;
#[cfg(feature = "jit")]
mod type_ops;
#[cfg(feature = "jit")]
mod globals;
#[cfg(feature = "jit")]
mod atom_ops;
#[cfg(feature = "jit")]
mod higher_order_ops;
#[cfg(feature = "jit")]
mod meta_ops;
#[cfg(feature = "jit")]
mod multi_return;

#[cfg(feature = "jit")]
pub use stack::compile_stack_op;
#[cfg(feature = "jit")]
pub use values::{compile_simple_value_op, compile_runtime_value_op, ValueHandlerContext};
#[cfg(feature = "jit")]
pub use arithmetic::{compile_simple_arithmetic_op, compile_pow, ArithmeticHandlerContext};
#[cfg(feature = "jit")]
pub use comparison::{compile_boolean_op, compile_comparison_op};
#[cfg(feature = "jit")]
pub use locals::compile_local_op;
#[cfg(feature = "jit")]
pub use type_predicates::compile_type_predicate_op;
#[cfg(feature = "jit")]
pub use math::{compile_extended_math_op, MathHandlerContext};
#[cfg(feature = "jit")]
pub use sexpr::{compile_sexpr_access_op, compile_sexpr_create_op, SExprHandlerContext};
#[cfg(feature = "jit")]
pub use expr::{compile_expr_op, ExprHandlerContext};
#[cfg(feature = "jit")]
pub use control_flow::{
    compile_return, compile_halt, compile_jump, compile_jump_short,
    compile_jump_if_false, compile_jump_if_false_short,
    compile_jump_if_true, compile_jump_if_true_short,
    compile_jump_if_nil, compile_jump_if_error, compile_jump_table,
};
#[cfg(feature = "jit")]
pub use calls::{
    compile_call, compile_tail_call, compile_call_n, compile_tail_call_n,
    compile_call_native, compile_call_external, compile_call_cached,
    CallHandlerContext,
};
#[cfg(feature = "jit")]
pub use nondet::{
    compile_fork, compile_yield, compile_collect, compile_cut, compile_guard,
    compile_amb, compile_commit, compile_backtrack, compile_fail,
    compile_begin_nondet, compile_end_nondet,
    NondetHandlerContext,
};
#[cfg(feature = "jit")]
pub use bindings::{
    compile_load_binding, compile_store_binding, compile_has_binding,
    compile_clear_bindings, compile_push_binding_frame, compile_pop_binding_frame,
    BindingHandlerContext,
};
#[cfg(feature = "jit")]
pub use pattern_matching::{
    compile_match, compile_match_bind, compile_match_head, compile_match_arity,
    compile_match_guard, compile_unify, compile_unify_bind,
    PatternMatchingHandlerContext,
};
#[cfg(feature = "jit")]
pub use space::{
    compile_space_add, compile_space_remove, compile_space_get_atoms, compile_space_match,
    compile_new_state, compile_get_state, compile_change_state,
    SpaceHandlerContext,
};
#[cfg(feature = "jit")]
pub use rules::{
    compile_dispatch_rules, compile_try_rule, compile_next_rule, compile_commit_rule,
    compile_fail_rule, compile_lookup_rules, compile_apply_subst, compile_define_rule,
    RulesHandlerContext,
};
#[cfg(feature = "jit")]
pub use special_forms::{
    compile_eval_if, compile_eval_let, compile_eval_let_star, compile_eval_match,
    compile_eval_case, compile_eval_chain, compile_eval_quote, compile_eval_unquote,
    compile_eval_eval, compile_eval_bind, compile_eval_new, compile_eval_collapse,
    compile_eval_superpose, compile_eval_memo, compile_eval_memo_first, compile_eval_pragma,
    compile_eval_function, compile_eval_lambda, compile_eval_apply,
    SpecialFormsHandlerContext,
};
#[cfg(feature = "jit")]
pub use mork::{
    compile_mork_lookup, compile_mork_match, compile_mork_insert, compile_mork_delete,
    MorkHandlerContext,
};
#[cfg(feature = "jit")]
pub use debug::{
    compile_trace, compile_breakpoint,
    DebugHandlerContext,
};
#[cfg(feature = "jit")]
pub use type_ops::{
    compile_get_type, compile_check_type, compile_assert_type,
    TypeOpsHandlerContext,
};
#[cfg(feature = "jit")]
pub use globals::{
    compile_load_global, compile_store_global, compile_load_space, compile_load_upvalue,
    GlobalsHandlerContext,
};
#[cfg(feature = "jit")]
pub use atom_ops::{
    compile_decon_atom, compile_repr,
    AtomOpsHandlerContext,
};
#[cfg(feature = "jit")]
pub use higher_order_ops::{
    compile_map_atom, compile_filter_atom, compile_foldl_atom,
    HigherOrderOpsHandlerContext,
};
#[cfg(feature = "jit")]
pub use meta_ops::{
    compile_get_metatype, compile_bloom_check,
    MetaOpsHandlerContext,
};
#[cfg(feature = "jit")]
pub use multi_return::{
    compile_return_multi, compile_collect_n,
    MultiReturnHandlerContext,
};
