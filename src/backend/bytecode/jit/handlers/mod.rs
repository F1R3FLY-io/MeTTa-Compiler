//! Opcode handlers for JIT compilation
//!
//! This module contains handlers for each category of bytecode opcodes.
//! Each handler module compiles specific opcodes to Cranelift IR.

mod stack;

mod values;

mod arithmetic;

mod comparison;

mod locals;

mod type_predicates;

mod math;

mod sexpr;

mod expr;

mod control_flow;

mod calls;

mod nondet;

mod bindings;

mod pattern_matching;

mod space;

mod rules;

mod special_forms;

mod mork;

mod debug;

mod type_ops;

mod globals;

mod atom_ops;

mod higher_order_ops;

mod meta_ops;

mod multi_return;

pub use stack::compile_stack_op;

pub use values::{compile_runtime_value_op, compile_simple_value_op, ValueHandlerContext};

pub use arithmetic::{compile_pow, compile_simple_arithmetic_op, ArithmeticHandlerContext};

pub use comparison::{compile_boolean_op, compile_comparison_op};

pub use locals::compile_local_op;

pub use type_predicates::compile_type_predicate_op;

pub use math::{compile_extended_math_op, MathHandlerContext};

pub use sexpr::{compile_sexpr_access_op, compile_sexpr_create_op, SExprHandlerContext};

pub use expr::{compile_expr_op, ExprHandlerContext};

pub use control_flow::{
    compile_halt, compile_jump, compile_jump_if_error, compile_jump_if_false,
    compile_jump_if_false_short, compile_jump_if_nil, compile_jump_if_true,
    compile_jump_if_true_short, compile_jump_short, compile_jump_table, compile_return,
};

pub use calls::{
    compile_call, compile_call_cached, compile_call_external, compile_call_n, compile_call_native,
    compile_tail_call, compile_tail_call_n, CallHandlerContext,
};

pub use nondet::{
    compile_amb, compile_backtrack, compile_begin_nondet, compile_collect, compile_commit,
    compile_cut, compile_end_nondet, compile_fail, compile_fork, compile_guard, compile_yield,
    NondetHandlerContext,
};

pub use bindings::{
    compile_clear_bindings, compile_has_binding, compile_load_binding, compile_pop_binding_frame,
    compile_push_binding_frame, compile_store_binding, BindingHandlerContext,
};

pub use pattern_matching::{
    compile_match, compile_match_arity, compile_match_bind, compile_match_guard,
    compile_match_head, compile_unify, compile_unify_bind, PatternMatchingHandlerContext,
};

pub use space::{
    compile_change_state, compile_get_state, compile_new_state, compile_space_add,
    compile_space_get_atoms, compile_space_match, compile_space_remove, SpaceHandlerContext,
};

pub use rules::{
    compile_apply_subst, compile_commit_rule, compile_define_rule, compile_dispatch_rules,
    compile_fail_rule, compile_lookup_rules, compile_next_rule, compile_try_rule,
    RulesHandlerContext,
};

pub use special_forms::{
    compile_eval_apply, compile_eval_bind, compile_eval_case, compile_eval_chain,
    compile_eval_collapse, compile_eval_eval, compile_eval_function, compile_eval_if,
    compile_eval_lambda, compile_eval_let, compile_eval_let_star, compile_eval_match,
    compile_eval_memo, compile_eval_memo_first, compile_eval_new, compile_eval_pragma,
    compile_eval_quote, compile_eval_superpose, compile_eval_unquote, SpecialFormsHandlerContext,
};

pub use mork::{
    compile_mork_delete, compile_mork_insert, compile_mork_lookup, compile_mork_match,
    MorkHandlerContext,
};

pub use debug::{compile_breakpoint, compile_trace, DebugHandlerContext};

pub use type_ops::{
    compile_assert_type, compile_check_type, compile_get_type, TypeOpsHandlerContext,
};

pub use globals::{
    compile_load_global, compile_load_space, compile_load_upvalue, compile_store_global,
    GlobalsHandlerContext,
};

pub use atom_ops::{compile_decon_atom, compile_repr, AtomOpsHandlerContext};

pub use higher_order_ops::{
    compile_filter_atom, compile_foldl_atom, compile_map_atom, HigherOrderOpsHandlerContext,
};

pub use meta_ops::{compile_bloom_check, compile_get_metatype, MetaOpsHandlerContext};

pub use multi_return::{compile_collect_n, compile_return_multi, MultiReturnHandlerContext};
