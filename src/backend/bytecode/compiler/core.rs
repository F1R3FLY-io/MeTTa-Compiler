//! Generic Bytecode Compiler for MeTTa expressions
//!
//! This module provides `GenericCompiler<V, F>` which compiles expressions of any
//! value type implementing `MettaValueTrait` to `GenericBytecodeChunk<V>`.
//!
//! This enables zero-conversion bytecode compilation for both heap-allocated
//! (`MettaValue`) and arena-allocated (`MettaValue`) values.

use std::sync::Arc;

use super::context::CompileContext;
use super::error::{CompileError, CompileResult};
use crate::backend::bytecode::chunk::{GenericBytecodeChunk, GenericChunkBuilder, JumpLabel};
use crate::backend::bytecode::opcodes::Opcode;
use crate::backend::eval::{is_eager_special_form, is_grounded_op};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Generic bytecode compiler that works with any value type.
///
/// # Type Parameters
///
/// - `V`: The value type (e.g., `MettaValue` or `MettaValue`)
/// - `F`: The factory type for constructing values
pub struct GenericCompiler<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// The chunk being built
    pub(crate) builder: GenericChunkBuilder<V, F>,
    /// Compilation context
    pub(crate) context: CompileContext,
    /// Factory for creating values
    pub(crate) factory: F,
    /// Current source line
    current_line: u32,
    /// Whether we're compiling in tail position (for TCO)
    pub(crate) in_tail_position: bool,
    /// Whether we're compiling inside a `(collapse ...)` body.
    /// When true, `compile_superpose` omits `Yield` — `CollapseEnd`
    /// drives backtracking via `op_fail_within_collapse` instead.
    pub(crate) in_collapse_scope: bool,
}

impl<V, F> GenericCompiler<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a new generic compiler
    pub fn new(name: impl Into<String>, factory: F) -> Self {
        let mut builder = GenericChunkBuilder::with_factory(name, factory.clone());
        builder.set_optimize(true);
        Self {
            builder,
            context: CompileContext::new(),
            factory,
            current_line: 1,
            in_tail_position: true,
            in_collapse_scope: false,
        }
    }

    /// Create a compiler with existing context (for nested functions)
    pub fn with_context(name: impl Into<String>, context: CompileContext, factory: F) -> Self {
        let mut builder = GenericChunkBuilder::with_factory(name, factory.clone());
        builder.set_optimize(true);
        Self {
            builder,
            context,
            factory,
            current_line: 1,
            in_tail_position: true,
            in_collapse_scope: false,
        }
    }

    /// Set the current source line
    pub fn set_line(&mut self, line: u32) {
        self.current_line = line;
        self.builder.set_line(line);
    }

    /// Compile a value expression
    pub fn compile(&mut self, expr: &V) -> CompileResult<()> {
        // Dispatch based on value type using trait methods
        if expr.is_unit() {
            self.builder.emit(Opcode::PushUnit);
            return Ok(());
        }

        if let Some(b) = expr.as_bool() {
            if b {
                self.builder.emit(Opcode::PushTrue);
            } else {
                self.builder.emit(Opcode::PushFalse);
            }
            return Ok(());
        }

        if let Some(n) = expr.as_long() {
            return self.compile_long(n);
        }

        if let Some(f) = expr.as_float() {
            return self.compile_float(f);
        }

        if let Some(s) = expr.as_string() {
            let val = self.factory.string(s);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushConstant, idx);
            return Ok(());
        }

        if let Some(name) = expr.as_atom() {
            return self.compile_atom(name);
        }

        if let Some(items) = expr.as_sexpr() {
            return self.compile_sexpr(items);
        }

        if expr.is_error() {
            // Compile error as-is (push the error value)
            let idx = self.builder.add_constant(expr.clone());
            self.builder.emit_u16(Opcode::PushConstant, idx);
            return Ok(());
        }

        // Fallback: push as constant
        let idx = self.builder.add_constant(expr.clone());
        self.builder.emit_u16(Opcode::PushConstant, idx);
        Ok(())
    }

    /// Compile a long integer
    fn compile_long(&mut self, n: i64) -> CompileResult<()> {
        if n >= -128 && n <= 127 {
            self.builder.emit_byte(Opcode::PushLongSmall, n as u8);
        } else {
            let val = self.factory.long(n);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushLong, idx);
        }
        Ok(())
    }

    /// Compile a float
    fn compile_float(&mut self, f: f64) -> CompileResult<()> {
        let val = self.factory.float(f);
        let idx = self.builder.add_constant(val);
        self.builder.emit_u16(Opcode::PushConstant, idx);
        Ok(())
    }

    /// Compile an atom (symbol or variable)
    fn compile_atom(&mut self, name: &str) -> CompileResult<()> {
        // Check if it's a variable (starts with $)
        if let Some(var_name) = name.strip_prefix('$') {
            // First try to resolve as local
            if let Some(slot) = self.context.resolve_local(var_name) {
                if slot <= 255 {
                    self.builder.emit_byte(Opcode::LoadLocal, slot as u8);
                } else {
                    self.builder.emit_u16(Opcode::LoadLocalWide, slot);
                }
                return Ok(());
            }

            // Try to resolve as upvalue
            if let Some(idx) = self.context.resolve_upvalue(var_name) {
                self.builder.emit_u16(Opcode::LoadUpvalue, idx);
                return Ok(());
            }

            // Variable not bound - push as symbol to be resolved at runtime
            let val = self.factory.atom(name);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushVariable, idx);
        } else {
            // Regular symbol
            let val = self.factory.atom(name);
            let idx = self.builder.add_constant(val);
            self.builder.emit_u16(Opcode::PushAtom, idx);
        }
        Ok(())
    }

    /// Compile an S-expression
    fn compile_sexpr(&mut self, items: &[V]) -> CompileResult<()> {
        if items.is_empty() {
            self.builder.emit(Opcode::PushEmpty);
            return Ok(());
        }

        // Check if the head is a known operation
        if let Some(head) = items.first() {
            if let Some(op_name) = head.as_atom() {
                // Try to compile as built-in operation
                if let Some(()) = self.try_compile_builtin(op_name, &items[1..])? {
                    return Ok(());
                }

                // Dynamic higher-order call: `($f arg...)`.
                // The VM/JIT already implement CallN/TailCallN; emitting it
                // here lets compiled rule bodies execute helpers such as PLN's
                // `(BestCandidate $rank ...)`, whose body calls
                // `($evaluateCandidateFunction $head)`.
                if op_name.starts_with('$') {
                    return self.compile_dynamic_call(head, &items[1..]);
                }

                // Not a builtin - check if it's a potential function call
                if !op_name.starts_with('&') {
                    return self.compile_call(op_name, &items[1..]);
                }
            }
        }

        // Fallback: compile as generic S-expression data
        for item in items {
            self.compile(item)?;
        }

        let arity = items.len();
        if arity <= 255 {
            self.builder.emit_byte(Opcode::MakeSExpr, arity as u8);
        } else {
            self.builder.emit_u16(Opcode::MakeSExprLarge, arity as u16);
        }

        Ok(())
    }

    /// Compile an expression as a LITERAL S-expression, never emitting a
    /// function call (`Call` opcode) for user-defined heads.
    ///
    /// Used by structural operations (`car-atom`, `cdr-atom`) that need to
    /// see the syntactic form of their argument — not its evaluation. The
    /// VM's `StructuralHead`/`StructuralTail` opcodes apply the 4-condition
    /// pre-eval predicate at runtime against the live environment, so the
    /// argument must arrive unreduced.
    ///
    /// Semantics:
    /// - S-expression: recursively compile each child as literal, then
    ///   emit `MakeSExpr` / `MakeSExprLarge` to reconstruct the list at
    ///   runtime. Never descends into `compile_call`.
    /// - Atom / variable / primitive: delegate to `self.compile`, which
    ///   correctly emits `PushAtom`, `LoadLocal`/`PushVariable`, or
    ///   `PushLong`/`PushConstant` — so bound variables like `$x` resolve
    ///   to their current values at runtime. For example:
    ///   `(let $x (f a b) (car-atom $x))` compiles `$x` as `LoadLocal N`,
    ///   the VM pushes the bound value `(f a b)`, then `StructuralHead`
    ///   applies the pre-eval predicate on `(f a b)` — identical to
    ///   tree-walker behavior.
    fn compile_as_literal_sexpr(&mut self, expr: &V) -> CompileResult<()> {
        if let Some(items) = expr.as_sexpr() {
            if items.is_empty() {
                self.builder.emit(Opcode::PushEmpty);
                return Ok(());
            }
            // Recursively compile each child as literal — never `compile_call`.
            for item in items {
                self.compile_as_literal_sexpr(item)?;
            }
            let arity = items.len();
            if arity <= 255 {
                self.builder.emit_byte(Opcode::MakeSExpr, arity as u8);
            } else {
                self.builder.emit_u16(Opcode::MakeSExprLarge, arity as u16);
            }
            return Ok(());
        }
        // Non-sexpr (atoms, variables, primitives): normal compilation is
        // correct — it resolves variables via LoadLocal, emits PushAtom for
        // symbols, and pushes primitives verbatim. None of these paths emit
        // `Call`, so variable bindings are preserved.
        self.compile(expr)
    }

    /// Compile a single argument to a user-defined call.
    ///
    /// **MeTTa HE parity.** HE's `interpret_function` only pre-evaluates an
    /// argument when the callee's DECLARED parameter type is concrete
    /// (non-meta). Unknown / meta / inferred-%Undefined% parameter types cause
    /// the argument to be passed unevaluated into unification-based rule
    /// matching. We can't decide that at compile time — declared types are
    /// registered during interpretation, and inferred types (Phase 10) don't
    /// appear in the registry until rules execute. So the compiler MUST NOT
    /// pre-reduce S-expr args whose head is user-defined: it constructs the
    /// arg as literal data, and `op_dispatch_rules` → `vm_type_driven_pre_eval`
    /// decides per-arg at runtime using the live type environment.
    ///
    /// Exception: grounded operators (`+`, `*`, `cons-atom`, …) and eager
    /// special forms (`collapse`, `reduce`, …) are always-eager in HE — they
    /// reduce before being passed to any caller. We preserve the existing
    /// fast-path for those heads: they compile through `self.compile`, which
    /// emits the direct builtin opcodes, avoiding a needless round-trip
    /// through the trampoline.
    fn compile_arg_for_user_call(&mut self, arg: &V) -> CompileResult<()> {
        if let Some(items) = arg.as_sexpr() {
            if let Some(head) = items.first().and_then(|v| v.as_atom()) {
                if is_grounded_op(head) || is_eager_special_form(head) {
                    return self.compile(arg);
                }
            }
        }
        self.compile_as_literal_sexpr(arg)
    }

    /// Compile a function call to a user-defined rule
    fn compile_call(&mut self, head: &str, args: &[V]) -> CompileResult<()> {
        let arity = args.len();

        // Compile arguments (left-to-right) - not in tail position.
        // User-defined call args go through `compile_arg_for_user_call` so
        // S-expr args with user-defined heads reach the VM as literal data,
        // letting `vm_type_driven_pre_eval` apply the HE meta-type rule at
        // runtime against the live env.
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        for arg in args {
            self.compile_arg_for_user_call(arg)?;
        }
        self.in_tail_position = saved_tail;

        // Add head symbol to constant pool
        let head_val = self.factory.atom(head);
        let head_index = self.builder.add_constant(head_val);

        if arity > 255 {
            return Err(CompileError::InvalidArityRange {
                op: head.to_string(),
                min: 0,
                max: 255,
                got: arity,
            });
        }

        if self.in_tail_position {
            self.builder.emit_u16(Opcode::TailCall, head_index);
        } else {
            self.builder.emit_u16(Opcode::Call, head_index);
        }
        self.builder.emit_raw(&[arity as u8]);

        Ok(())
    }

    /// Compile a function call whose head is supplied at runtime.
    ///
    /// Stack contract for `CallN`/`TailCallN` is `[head, arg1, ..., argN]`.
    /// Arguments use the same HE-parity path as constant-head user calls:
    /// user-headed S-expression args are materialized as data, while grounded
    /// and eager special-form args are reduced.
    fn compile_dynamic_call(&mut self, head: &V, args: &[V]) -> CompileResult<()> {
        let arity = args.len();
        if arity > 255 {
            return Err(CompileError::InvalidArityRange {
                op: head.as_atom().unwrap_or("<dynamic>").to_string(),
                min: 0,
                max: 255,
                got: arity,
            });
        }

        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(head)?;
        for arg in args {
            self.compile_arg_for_user_call(arg)?;
        }
        self.in_tail_position = saved_tail;

        if self.in_tail_position {
            self.builder.emit_byte(Opcode::TailCallN, arity as u8);
        } else {
            self.builder.emit_byte(Opcode::CallN, arity as u8);
        }

        Ok(())
    }

    /// Try to compile a built-in operation
    fn try_compile_builtin(&mut self, op: &str, args: &[V]) -> CompileResult<Option<()>> {
        match op {
            // Arithmetic operations
            "+" => {
                self.check_arity("+", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Add);
                Ok(Some(()))
            }
            "-" => {
                match args.len() {
                    1 => {
                        // Unary minus: (- x) => neg(x)
                        self.compile(&args[0])?;
                        self.builder.emit(Opcode::Neg);
                    }
                    2 => {
                        // Binary minus: (- a b) => a - b
                        self.compile(&args[0])?;
                        self.compile(&args[1])?;
                        self.builder.emit(Opcode::Sub);
                    }
                    _ => {
                        return Err(CompileError::InvalidArityRange {
                            op: "-".to_string(),
                            min: 1,
                            max: 2,
                            got: args.len(),
                        });
                    }
                }
                Ok(Some(()))
            }
            "*" => {
                self.check_arity("*", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Mul);
                Ok(Some(()))
            }
            "/" => {
                self.check_arity("/", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Div);
                Ok(Some(()))
            }
            "%" | "mod" => {
                self.check_arity("%", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Mod);
                Ok(Some(()))
            }
            "pow" | "pow-math" => {
                self.check_arity("pow", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Pow);
                Ok(Some(()))
            }
            "abs" | "abs-math" => {
                self.check_arity("abs", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Abs);
                Ok(Some(()))
            }
            "neg" => {
                self.check_arity("neg", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Neg);
                Ok(Some(()))
            }
            "floor-div" => {
                self.check_arity("floor-div", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::FloorDiv);
                Ok(Some(()))
            }

            // Comparison operations
            "<" => {
                self.check_arity("<", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Lt);
                Ok(Some(()))
            }
            "<=" => {
                self.check_arity("<=", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Le);
                Ok(Some(()))
            }
            ">" => {
                self.check_arity(">", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Gt);
                Ok(Some(()))
            }
            ">=" => {
                self.check_arity(">=", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Ge);
                Ok(Some(()))
            }
            "==" => {
                self.check_arity("==", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Eq);
                Ok(Some(()))
            }
            "!=" => {
                self.check_arity("!=", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Ne);
                Ok(Some(()))
            }

            // Boolean operations
            "and" => {
                self.check_arity("and", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::And);
                Ok(Some(()))
            }
            "or" => {
                self.check_arity("or", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Or);
                Ok(Some(()))
            }
            "not" => {
                self.check_arity("not", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Not);
                Ok(Some(()))
            }
            "xor" => {
                self.check_arity("xor", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Xor);
                Ok(Some(()))
            }

            // Control flow
            "if" => {
                self.compile_if(args)?;
                Ok(Some(()))
            }
            "if-reducible" => {
                self.check_arity("if-reducible", args.len(), 3)?;
                // Native if-reducible: compile expr, compare result to original,
                // branch to then (reduced) or else (irreducible).
                // No trampoline delegation — the VM does everything inline.

                // Save original expression as constant (for comparison after eval)
                let original_idx = self.builder.add_constant(args[0].clone());

                // Compile the expression — generates evaluation bytecode
                let saved_tail = self.in_tail_position;
                self.in_tail_position = false;
                self.compile(&args[0])?;
                self.in_tail_position = saved_tail;

                // Push unevaluated original for comparison
                self.builder.emit_u16(Opcode::PushConstant, original_idx);

                // JumpIfIdentical: pops both values, jumps if result == original
                // (irreducible → else branch)
                let else_jump = self.builder.emit_jump(Opcode::JumpIfIdentical);

                // Then branch (expression reduced)
                self.compile(&args[1])?;
                let end_jump = self.builder.emit_jump(Opcode::Jump);

                // Else branch (expression irreducible)
                self.builder.patch_jump(else_jump);
                self.compile(&args[2])?;
                self.builder.patch_jump(end_jump);

                Ok(Some(()))
            }
            "if-equal" => {
                self.check_arity("if-equal", args.len(), 4)?;
                // Push all 4 args as unevaluated constants.
                // VM's op_eval_if_equal does alpha-equiv on raw values.
                for arg in args {
                    let idx = self.builder.add_constant(arg.clone());
                    self.builder.emit_u16(Opcode::PushConstant, idx);
                }
                self.builder.emit(Opcode::EvalIfEqual);
                Ok(Some(()))
            }
            "match" => {
                if !(args.len() == 3 || args.len() == 4) {
                    return Err(CompileError::InvalidArityRange {
                        op: "match".to_string(),
                        min: 3,
                        max: 4,
                        got: args.len(),
                    });
                }
                // Native path for &self: MatchSelf opcode calls env.match_space() directly
                if args[0].as_atom() == Some("&self") {
                    let pattern_idx = self.builder.add_constant(args[1].clone());
                    self.builder.emit_u16(Opcode::PushConstant, pattern_idx);
                    let template_idx = self.builder.add_constant(args[2].clone());
                    self.builder.emit_u16(Opcode::PushConstant, template_idx);
                    self.builder.emit(Opcode::MatchSelf);
                    return Ok(Some(()));
                }
                // Phase E: native path for external/named spaces.
                // Stack: [space_ref, pattern, template] → [result(s)]
                // compile(space_ref) evaluates the space expression; pattern + template
                // are pushed as constants (preserving free vars for unification).
                self.compile(&args[0])?;
                let pattern_idx = self.builder.add_constant(args[1].clone());
                self.builder.emit_u16(Opcode::PushConstant, pattern_idx);
                let template_idx = self.builder.add_constant(args[2].clone());
                self.builder.emit_u16(Opcode::PushConstant, template_idx);
                self.builder.emit(Opcode::MatchExternal);
                Ok(Some(()))
            }
            "match-or" => {
                self.check_arity("match-or", args.len(), 4)?;
                // Native path for &self: MatchSelfOr with default fallback
                if args[0].as_atom() == Some("&self") {
                    let pattern_idx = self.builder.add_constant(args[1].clone());
                    self.builder.emit_u16(Opcode::PushConstant, pattern_idx);
                    let default_idx = self.builder.add_constant(args[2].clone());
                    self.builder.emit_u16(Opcode::PushConstant, default_idx);
                    let template_idx = self.builder.add_constant(args[3].clone());
                    self.builder.emit_u16(Opcode::PushConstant, template_idx);
                    self.builder.emit(Opcode::MatchSelfOr);
                    return Ok(Some(()));
                }
                // Phase E: native path for external/named spaces.
                // Stack: [space_ref, pattern, template, default]
                self.compile(&args[0])?;
                let pattern_idx = self.builder.add_constant(args[1].clone());
                self.builder.emit_u16(Opcode::PushConstant, pattern_idx);
                let default_idx = self.builder.add_constant(args[2].clone());
                self.builder.emit_u16(Opcode::PushConstant, default_idx);
                let template_idx = self.builder.add_constant(args[3].clone());
                self.builder.emit_u16(Opcode::PushConstant, template_idx);
                self.builder.emit(Opcode::MatchExternalOr);
                Ok(Some(()))
            }
            // Phase A: native 4-arg `(unify val1 pattern2 success failure)`.
            // Emit layout:
            //   compile(val1)                    ; stack: [val1]
            //   compile_quoted(pattern2)         ; stack: [val1, pattern2]
            //   Unify4 fail_off                  ; unify; on success install bindings
            //   <success body>
            //   Jump done_off
            //   fail_label: <failure body>
            //   done:
            "unify" => {
                self.check_arity("unify", args.len(), 4)?;
                // 1. Compile val1 (may evaluate, yields value on stack).
                self.compile(&args[0])?;
                // 2. Push pattern2 as a quoted constant so free vars survive.
                self.compile_quoted(&args[1])?;
                // 3. Emit Unify4 with placeholder fail_offset (patched below).
                let fail_label = self.builder.emit_jump(Opcode::Unify4);
                // 4. Compile success body (executes with bindings installed).
                self.compile(&args[2])?;
                // 5. Jump over the failure body to done.
                let done_label = self.builder.emit_jump(Opcode::Jump);
                // 6. Failure label: patch Unify4's fail_offset here.
                self.builder.patch_jump(fail_label);
                // 7. Compile failure body.
                self.compile(&args[3])?;
                // 8. Patch the success→done jump.
                self.builder.patch_jump(done_label);
                Ok(Some(()))
            }
            // Phase C: native `(collapse-bind expr)`.
            // Emit layout:
            //   CollapseBindBegin tracked_vars_idx
            //   compile(expr)
            //   CollapseBindEnd
            "collapse-bind" => {
                self.check_arity("collapse-bind", args.len(), 1)?;
                let tracked_vars = args[0]
                    .free_variables()
                    .into_iter()
                    .map(|name| self.factory.atom(name))
                    .collect();
                let tracked_vars_value = self.factory.sexpr(tracked_vars);
                let tracked_idx = self.builder.add_constant(tracked_vars_value);
                self.builder
                    .emit_u16(Opcode::CollapseBindBegin, tracked_idx);
                self.compile(&args[0])?;
                self.builder.emit(Opcode::CollapseBindEnd);
                Ok(Some(()))
            }

            // Binding forms
            "let" => {
                self.compile_let(args)?;
                Ok(Some(()))
            }
            "let*" => {
                self.compile_let_star(args)?;
                Ok(Some(()))
            }

            // Quote and eval
            "quote" => {
                self.check_arity("quote", args.len(), 1)?;
                self.compile_quoted(&args[0])?;
                Ok(Some(()))
            }
            "eval" => {
                self.check_arity("eval", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::EvalEval);
                Ok(Some(()))
            }
            "unquote" => {
                self.check_arity("unquote", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::EvalUnquote);
                Ok(Some(()))
            }

            // Force evaluation
            "!" => {
                self.check_arity("!", args.len(), 1)?;
                self.compile(&args[0])?;
                Ok(Some(()))
            }

            // Type operations
            "get-type" => {
                self.check_arity("get-type", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetType);
                Ok(Some(()))
            }
            "check-type" => {
                self.check_arity("check-type", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::CheckType);
                Ok(Some(()))
            }
            "get-metatype" => {
                self.check_arity("get-metatype", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetMetaType);
                Ok(Some(()))
            }
            "validate-atom" => {
                self.check_arity("validate-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::ValidateAtom);
                Ok(Some(()))
            }
            "get-type-space" => {
                self.check_arity("get-type-space", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::GetTypeSpace);
                Ok(Some(()))
            }
            "is-function" => {
                self.check_arity("is-function", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::IsFunction);
                Ok(Some(()))
            }
            "type-cast" => {
                self.check_arity("type-cast", args.len(), 3)?;
                self.compile(&args[0])?; // atom
                self.compile(&args[1])?; // expected type
                self.compile(&args[2])?; // space
                self.builder.emit(Opcode::TypeCast);
                Ok(Some(()))
            }

            // Case dispatch — jump-based inline pattern matching
            "case" => {
                self.compile_case(args)?;
                return Ok(Some(()));
            }

            // Nondeterminism
            "superpose" => {
                self.compile_superpose(args)?;
                Ok(Some(()))
            }
            "collapse" => {
                self.check_arity("collapse", args.len(), 1)?;
                // Native collapse: CollapseBegin saves nondeterministic context,
                // body compiles with in_collapse_scope (superpose omits Yield),
                // CollapseEnd collects results and drives backtracking.
                let collapse_jump = self.builder.emit_jump(Opcode::CollapseBegin);
                let saved_collapse = self.in_collapse_scope;
                let saved_tail = self.in_tail_position;
                self.in_collapse_scope = true;
                self.in_tail_position = false;
                self.compile(&args[0])?;
                self.in_collapse_scope = saved_collapse;
                self.in_tail_position = saved_tail;
                self.builder.emit(Opcode::CollapseEnd);
                self.builder.patch_jump(collapse_jump);
                Ok(Some(()))
            }

            // List operations
            //
            // car-atom/cdr-atom use StructuralHead/StructuralTail to preserve
            // the raw (unreduced) argument through to the VM, which then
            // applies the 4-condition pre-eval predicate against the live
            // environment — identical semantics to the tree-walker's
            // `is_reducible_structural_arg` (src/backend/eval/step/sexpr.rs).
            //
            // The argument is compiled via `compile_as_literal_sexpr`, which
            // recursively constructs the s-expression at runtime via
            // `MakeSExpr` (never emitting `Call` for user-defined function
            // heads). This preserves the syntactic structure while still
            // allowing variables (`$x`) to be resolved via `LoadLocal`, so
            // e.g. `(let $x (grandfather a b) (car-atom $x))` correctly
            // substitutes $x before the structural op sees it.
            "car-atom" => {
                self.check_arity("car-atom", args.len(), 1)?;
                self.compile_as_literal_sexpr(&args[0])?;
                self.builder.emit(Opcode::StructuralHead);
                Ok(Some(()))
            }
            "cdr-atom" => {
                self.check_arity("cdr-atom", args.len(), 1)?;
                self.compile_as_literal_sexpr(&args[0])?;
                self.builder.emit(Opcode::StructuralTail);
                Ok(Some(()))
            }
            "cons-atom" => {
                self.check_arity("cons-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ConsAtom);
                Ok(Some(()))
            }
            "size-atom" => {
                self.check_arity("size-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetArity);
                Ok(Some(()))
            }
            "empty" => {
                self.check_arity("empty", args.len(), 0)?;
                self.builder.emit(Opcode::Fail);
                Ok(Some(()))
            }
            "decons-atom" => {
                self.check_arity("decons-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::DeconsAtom);
                Ok(Some(()))
            }
            "repr" => {
                self.check_arity("repr", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Repr);
                Ok(Some(()))
            }

            // Chain operation
            "chain" => {
                self.compile_chain(args)?;
                Ok(Some(()))
            }

            // Error handling
            "error" => {
                self.check_arity("error", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit_byte(Opcode::MakeSExpr, 3);
                Ok(Some(()))
            }
            "is-error" => {
                self.check_arity("is-error", args.len(), 1)?;
                self.compile(&args[0])?;
                let not_error = self.builder.emit_jump(Opcode::JumpIfError);
                self.builder.emit(Opcode::PushFalse);
                let done = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(not_error);
                self.builder.emit(Opcode::PushTrue);
                self.builder.patch_jump(done);
                Ok(Some(()))
            }
            "catch" => {
                self.check_arity("catch", args.len(), 2)?;
                self.compile(&args[0])?;
                let no_error = self.builder.emit_jump(Opcode::JumpIfError);
                let done = self.builder.emit_jump(Opcode::Jump);
                self.builder.patch_jump(no_error);
                self.builder.emit(Opcode::Pop);
                self.compile(&args[1])?;
                self.builder.patch_jump(done);
                Ok(Some(()))
            }

            // Space operations
            "new-space" => {
                self.check_arity("new-space", args.len(), 0)?;
                self.builder.emit(Opcode::EvalNew);
                Ok(Some(()))
            }
            "add-atom" => {
                self.check_arity("add-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                // MeTTa HE: add-atom's atom arg is unevaluated data. Compiling
                // as a literal prevents sub-expressions like `(= lhs rhs)` from
                // being re-fired as rule definitions (which would double-add
                // the rule under bytecode + trampoline fallback).
                self.compile_quoted(&args[1])?;
                self.builder.emit(Opcode::SpaceAdd);
                Ok(Some(()))
            }
            "remove-atom" => {
                self.check_arity("remove-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                // MeTTa HE: remove-atom's atom arg is unevaluated data. See
                // add-atom above for the rationale.
                self.compile_quoted(&args[1])?;
                self.builder.emit(Opcode::SpaceRemove);
                Ok(Some(()))
            }
            "get-atoms" => {
                self.check_arity("get-atoms", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::SpaceGetAtoms);
                Ok(Some(()))
            }

            // State operations
            "new-state" => {
                self.check_arity("new-state", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::NewState);
                Ok(Some(()))
            }
            "get-state" => {
                self.check_arity("get-state", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::GetState);
                Ok(Some(()))
            }
            "change-state!" => {
                self.check_arity("change-state!", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ChangeState);
                Ok(Some(()))
            }

            // Rule definition: (= pattern body)
            // Emit DefineRule opcode which pops body then pattern,
            // calls env.add_rule(pattern, body), and pushes Unit.
            // Pop the Unit to match tree-walker semantics (empty result).
            "=" => {
                self.check_arity("=", args.len(), 2)?;
                self.compile_quoted(&args[0])?; // Push pattern
                self.compile_quoted(&args[1])?; // Push body
                self.builder.emit(Opcode::DefineRule);
                self.builder.emit(Opcode::Pop); // Discard Unit — rule defs return empty
                Ok(Some(()))
            }

            // I/O operations
            "println!" => {
                self.check_arity("println!", args.len(), 1)?;
                self.compile(&args[0])?;
                let print_val = self.factory.atom("println!");
                let idx = self.builder.add_constant(print_val);
                self.builder.emit_u16(Opcode::PushAtom, idx);
                self.builder.emit(Opcode::Swap);
                self.builder.emit_byte(Opcode::MakeSExpr, 2);
                Ok(Some(()))
            }
            "trace!" => {
                self.check_arity("trace!", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Trace);
                Ok(Some(()))
            }

            // nop
            "nop" => {
                self.check_arity("nop", args.len(), 0)?;
                self.builder.emit(Opcode::PushUnit);
                Ok(Some(()))
            }

            // Math operations
            "sqrt-math" => {
                self.check_arity("sqrt-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Sqrt);
                Ok(Some(()))
            }
            "log-math" => {
                self.check_arity("log-math", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Log);
                Ok(Some(()))
            }
            "trunc-math" => {
                self.check_arity("trunc-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Trunc);
                Ok(Some(()))
            }
            "ceil-math" => {
                self.check_arity("ceil-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Ceil);
                Ok(Some(()))
            }
            "floor-math" => {
                self.check_arity("floor-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::FloorMath);
                Ok(Some(()))
            }
            "round-math" => {
                self.check_arity("round-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Round);
                Ok(Some(()))
            }
            "sin-math" => {
                self.check_arity("sin-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Sin);
                Ok(Some(()))
            }
            "cos-math" => {
                self.check_arity("cos-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Cos);
                Ok(Some(()))
            }
            "tan-math" => {
                self.check_arity("tan-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Tan);
                Ok(Some(()))
            }
            "asin-math" => {
                self.check_arity("asin-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Asin);
                Ok(Some(()))
            }
            "acos-math" => {
                self.check_arity("acos-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Acos);
                Ok(Some(()))
            }
            "atan-math" => {
                self.check_arity("atan-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Atan);
                Ok(Some(()))
            }
            "isnan-math" => {
                self.check_arity("isnan-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::IsNan);
                Ok(Some(()))
            }
            "isinf-math" => {
                self.check_arity("isinf-math", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::IsInf);
                Ok(Some(()))
            }

            // Expression manipulation
            "index-atom" => {
                self.check_arity("index-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::IndexAtom);
                Ok(Some(()))
            }
            "min-atom" => {
                self.check_arity("min-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::MinAtom);
                Ok(Some(()))
            }
            "max-atom" => {
                self.check_arity("max-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::MaxAtom);
                Ok(Some(()))
            }
            // X.5e — binary min/max compiled by wrapping operands in a 2-tuple
            // and reusing MinAtom/MaxAtom. T0 has dedicated MinOp/MaxOp at
            // grounded/arithmetic.rs:498-660 with the same shape; T1 had only
            // the tuple form. Wrapping is cheap (one MakeSExpr) and yields
            // identical results across tiers.
            "min" => {
                self.check_arity("min", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit_byte(Opcode::MakeSExpr, 2);
                self.builder.emit(Opcode::MinAtom);
                Ok(Some(()))
            }
            "max" => {
                self.check_arity("max", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit_byte(Opcode::MakeSExpr, 2);
                self.builder.emit(Opcode::MaxAtom);
                Ok(Some(()))
            }

            // Set operations
            "unique-atom" => {
                self.check_arity("unique-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::UniqueAtom);
                Ok(Some(()))
            }
            // Explicit alias of `unique-atom` — both use alpha-equivalence
            // (matching MeTTa HE).
            "alpha-unique-atom" => {
                self.check_arity("alpha-unique-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::AlphaUniqueAtom);
                Ok(Some(()))
            }
            // PeTTa-compatible structural-equality dedup. Distinct from
            // `unique-atom` (which uses alpha-equivalence).
            "struct-unique-atom" => {
                self.check_arity("struct-unique-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::StructUniqueAtom);
                Ok(Some(()))
            }
            "union-atom" => {
                self.check_arity("union-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::UnionAtom);
                Ok(Some(()))
            }
            "intersection-atom" => {
                self.check_arity("intersection-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::IntersectionAtom);
                Ok(Some(()))
            }
            "subtraction-atom" => {
                self.check_arity("subtraction-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::SubtractionAtom);
                Ok(Some(()))
            }

            // Tuple operations
            "tuple-concat" => {
                self.check_arity("tuple-concat", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::TupleConcat);
                Ok(Some(()))
            }
            "tuple-count" => {
                self.check_arity("tuple-count", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::TupleCount);
                Ok(Some(()))
            }
            "without" => {
                self.check_arity("without", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Without);
                Ok(Some(()))
            }
            "element-of" => {
                self.check_arity("element-of", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ElementOf);
                Ok(Some(()))
            }

            // PeTTa-compatible aliases — see src/backend/eval/list_ops/ops.rs
            // for the matching tree-walker implementations.
            //
            // `is-member`: alias of `element-of` (same arg order: elem first, list second).
            "is-member" => {
                self.check_arity("is-member", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ElementOf);
                Ok(Some(()))
            }
            // `append`: alias of `tuple-concat`.
            "append" => {
                self.check_arity("append", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::TupleConcat);
                Ok(Some(()))
            }
            // `length`: alias of `tuple-count` / `size-atom`.
            "length" => {
                self.check_arity("length", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::TupleCount);
                Ok(Some(()))
            }
            // `exclude-item`: like `without` but with reversed arg order.
            // PeTTa: (exclude-item elem tuple). MeTTaTron `without`: (without tuple elem).
            // We swap operands at compile time and emit the existing Without opcode.
            "exclude-item" => {
                self.check_arity("exclude-item", args.len(), 2)?;
                // Compile in swapped order so the stack has [tuple, elem] for Without.
                self.compile(&args[1])?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Without);
                Ok(Some(()))
            }
            // `msort`: numeric ascending sort. New opcode.
            "msort" => {
                self.check_arity("msort", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::Msort);
                Ok(Some(()))
            }
            // `reduce`: PeTTa-compatible alias of `eval`. The argument is
            // already evaluated by applicative-order pre-evaluation, so this
            // is effectively the identity at the bytecode level. Emitting
            // EvalEval gives the strongest semantics (forces re-evaluation).
            "reduce" => {
                self.check_arity("reduce", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::EvalEval);
                Ok(Some(()))
            }
            // `cut`: PLN/PeTTa no-op returning Unit. NOT to be confused with
            // `Opcode::Cut` (the nondeterminism cut at 0xF2). The PLN cut is
            // a 0-arg expression that produces Unit. Compile as PushUnit.
            "cut" => {
                self.check_arity("cut", args.len(), 0)?;
                self.builder.emit(Opcode::PushUnit);
                Ok(Some(()))
            }
            // `progn` (PeTTa): sequential evaluation, returns last value.
            // Compile-time desugar to nested `(let $_ a (let $_ b ...))`,
            // mirroring the tree-walker special-form handling. Reuses the
            // existing let compilation infrastructure entirely.
            "progn" => {
                if args.is_empty() {
                    return Err(CompileError::InvalidArity {
                        op: "progn".to_string(),
                        expected: 1,
                        got: 0,
                    });
                }
                if args.len() == 1 {
                    return self.compile(&args[0]).map(Some);
                }
                // Build the nested let from the right.
                let unused = self.factory.atom("$_progn_unused");
                let mut body = args.last().unwrap().clone();
                for arg in args[..args.len() - 1].iter().rev() {
                    body = self.factory.sexpr(vec![
                        self.factory.atom("let"),
                        unused.clone(),
                        arg.clone(),
                        body,
                    ]);
                }
                self.compile(&body).map(Some)
            }
            // `foldl-atom` PeTTa 3-arg form: (foldl-atom tuple init func)
            //
            // Phase 1b-C (HE-bisimilarity): translate to the 5-arg form
            // `(foldl-atom tuple init $__fa_acc $__fa_item (func $__fa_acc $__fa_item))`
            // which routes through Opcode::FoldlAtom (compiled at
            // iterative.rs). The previous compile-time unroll
            // `(f (f (f i a) b) c)` is incorrect: the unrolled form
            // evaluates each iteration's arg via the VM's applicative
            // pre-eval, which does NOT thread bindings across arguments.
            // Premise lists with shared free variables (e.g. PLN's
            // `((father $a $b) (father $b $c))`) produced spurious
            // derivations because iteration 2's `$b` re-bound
            // independently of iteration 1's.
            //
            // `Opcode::FoldlAtom` (with the Phase 1b-C binding-threading
            // fix in `op_foldl_atom`) correctly threads `acc_bindings`
            // across iterations via `apply_bindings_generic` before each
            // template dispatch, matching HE's recursive `foldl-atom`
            // semantics.
            "foldl-atom" if args.len() == 3 => {
                let list_arg = args[0].clone();
                let init = args[1].clone();
                let func = args[2].clone();
                // H15 (2026-05-05): per-call freshened epoch via
                // `intern_fresh_name`. The `$__fr_<epoch>_*` prefix
                // interlocks with the existing propagate_keys filter
                // (`!name.starts_with("$__fr_")`) so per-iter accumulator
                // names don't leak across nested fold scopes. Earlier
                // literal `$__fa_acc` / `$__fa_item` strings caused
                // cross-contamination in nested foldl-atom calls. The
                // sexpr.rs tree-walker site mirrors this scheme.
                let epoch = crate::backend::eval::freshening::allocate_epoch();
                let acc_var_name =
                    crate::backend::eval::freshening::intern_fresh_name(epoch, "fa_acc");
                let item_var_name =
                    crate::backend::eval::freshening::intern_fresh_name(epoch, "fa_item");
                let acc_var = self.factory.atom(acc_var_name);
                let item_var = self.factory.atom(item_var_name);
                let operation = self
                    .factory
                    .sexpr(vec![func, acc_var.clone(), item_var.clone()]);
                let foldl_sym = self.factory.atom("foldl-atom");
                let five_arg = self.factory.sexpr(vec![
                    foldl_sym, list_arg, init, acc_var, item_var, operation,
                ]);
                return self.compile(&five_arg).map(Some);
            }

            // Additional list operations (MeTTaTron extensions)
            "range" => {
                self.check_arity("range", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::Range);
                Ok(Some(()))
            }
            "reverse-atom" => {
                self.check_arity("reverse-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::ReverseAtom);
                Ok(Some(()))
            }
            "flatten-atom" => {
                self.check_arity("flatten-atom", args.len(), 1)?;
                self.compile(&args[0])?;
                self.builder.emit(Opcode::FlattenAtom);
                Ok(Some(()))
            }
            "zip-atom" => {
                self.check_arity("zip-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::ZipAtom);
                Ok(Some(()))
            }
            "take-atom" => {
                self.check_arity("take-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::TakeAtom);
                Ok(Some(()))
            }
            "drop-atom" => {
                self.check_arity("drop-atom", args.len(), 2)?;
                self.compile(&args[0])?;
                self.compile(&args[1])?;
                self.builder.emit(Opcode::DropAtom);
                Ok(Some(()))
            }

            // sort-tuple and best-candidate intentionally fall through to tree-walker.
            // They have complex iterative evaluation requiring full trampoline context.
            // /safe and clamp are handled by GroundedOperationTCO without opcodes.

            // Not a built-in
            _ => Ok(None),
        }
    }

    /// Compile an if expression
    ///
    /// MeTTa HE semantics: only Bool(true) → then, Bool(false) → else.
    /// Non-boolean conditions (including Unit, atoms, numbers) return
    /// unreduced `(if cond then else)`.
    ///
    /// Bytecode layout:
    ///   [condition]
    ///   JumpIfNotBool → non_bool_handler  (peek — condition stays on stack)
    ///   JumpIfFalse → else_branch         (pop — consumes condition)
    ///   [then_branch]
    ///   Jump → end
    /// non_bool_handler:                   (condition still on TOS from peek)
    ///   Pop                               (discard condition — we rebuild it as constant)
    ///   PushConstant (if cond then else)  (the unreduced S-expression)
    ///   Jump → end
    /// else_branch:
    ///   [else_branch]
    /// end:
    fn compile_if(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 2 || args.len() > 3 {
            return Err(CompileError::InvalidArityRange {
                op: "if".to_string(),
                min: 2,
                max: 3,
                got: args.len(),
            });
        }

        // Compile condition (not in tail position)
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(&args[0])?;
        self.in_tail_position = saved_tail;

        // MeTTa HE: non-boolean conditions return unreduced (if cond then else).
        // JumpIfNotBool peeks (doesn't pop) — condition stays on stack for the
        // non-bool handler. If condition IS bool, fall through to JumpIfFalse.
        let non_bool_jump = self.builder.emit_jump(Opcode::JumpIfNotBool);

        // Only Bool values reach here. JumpIfFalse pops and branches.
        let else_jump = self.builder.emit_jump(Opcode::JumpIfFalse);

        // Compile then branch (in tail position if we're in tail position)
        self.compile(&args[1])?;
        let end_jump = self.builder.emit_jump(Opcode::Jump);

        // Non-bool handler: condition is still on TOS (JumpIfNotBool peeked).
        // Pop it and push the unreduced (if cond then else) as a constant.
        // Since we can't know the runtime condition value at compile time, we
        // fall back to the tree-walker for non-boolean conditions by returning
        // an error that triggers the fallback.
        //
        // Actually, the simplest correct approach: just fall back to tree-walker
        // for any `if` with non-boolean conditions. The bytecode path handles
        // the common case (boolean conditions) efficiently.
        //
        // To achieve this, emit a Halt opcode in the non-bool handler which
        // causes the VM to return an error, triggering tree-walker fallback.
        self.builder.patch_jump(non_bool_jump);
        self.builder.emit(Opcode::Halt);

        // Else branch
        self.builder.patch_jump(else_jump);
        if args.len() == 3 {
            self.compile(&args[2])?;
        } else {
            self.builder.emit(Opcode::PushUnit);
        }

        self.builder.patch_jump(end_jump);
        Ok(())
    }

    /// Compile a let expression
    fn compile_let(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 3 {
            return Err(CompileError::InvalidArityRange {
                op: "let".to_string(),
                min: 3,
                max: usize::MAX,
                got: args.len(),
            });
        }

        // (let pattern value body)
        let pattern = &args[0];
        let value = &args[1];
        let body = &args[2];

        // Compile the value (not in tail position)
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(value)?;
        self.in_tail_position = saved_tail;

        // Bind the pattern
        self.context.begin_scope();
        self.bind_pattern(pattern)?;

        // Compile the body (in original tail position)
        self.compile(body)?;

        // Bug-Fix Phase 2b (2026-04): no cleanup emission needed. The VM
        // now stores locals in a dedicated `self.locals` vector disjoint
        // from the operand stack, so StoreLocal/LoadLocal leave the operand
        // stack clean. Previously we emitted `Swap; Pop × local_count` to
        // clear the pre-allocated-local-slot overlap, which had side-effect
        // of moving the body result INTO the slot and zeroing stack growth.
        let _local_count = self.context.end_scope();

        Ok(())
    }

    /// Compile a let* expression
    fn compile_let_star(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 2 {
            return Err(CompileError::InvalidArityRange {
                op: "let*".to_string(),
                min: 2,
                max: usize::MAX,
                got: args.len(),
            });
        }

        // (let* ((var1 val1) (var2 val2) ...) body)
        let bindings = &args[0];
        let body = &args[1];

        self.context.begin_scope();

        // Process bindings
        if let Some(items) = bindings.as_sexpr() {
            let saved_tail = self.in_tail_position;
            self.in_tail_position = false;

            for binding in items {
                if let Some(pair) = binding.as_sexpr() {
                    if pair.len() == 2 {
                        // Compile value
                        self.compile(&pair[1])?;
                        // Bind pattern
                        self.bind_pattern(&pair[0])?;
                    }
                }
            }

            self.in_tail_position = saved_tail;
        }

        // Compile body
        self.compile(body)?;

        // Bug-Fix Phase 2b (2026-04): no cleanup emission — see compile_let.
        let _local_count = self.context.end_scope();

        Ok(())
    }

    /// Bind a pattern to the value on stack top
    fn bind_pattern(&mut self, pattern: &V) -> CompileResult<()> {
        if let Some(name) = pattern.as_atom() {
            if let Some(var_name) = name.strip_prefix('$') {
                // Variable binding - declare local AND emit StoreLocal
                let slot = self.context.declare_local(var_name.to_string())?;
                if slot <= 255 {
                    self.builder.emit_byte(Opcode::StoreLocal, slot as u8);
                } else {
                    self.builder.emit_u16(Opcode::StoreLocalWide, slot);
                }
                return Ok(());
            }
            // Wildcard - just pop the value (both `_` and `$_`)
            if name == "_" || name == "$_" {
                self.builder.emit(Opcode::Pop);
                return Ok(());
            }
        }
        // Handle destructuring patterns (S-expressions)
        if let Some(items) = pattern.as_sexpr() {
            // For each element, dup the value, extract element, bind
            for (i, item) in items.iter().enumerate() {
                self.builder.emit(Opcode::Dup);
                self.builder.emit_byte(Opcode::GetElement, i as u8);
                self.bind_pattern(item)?;
            }
            // Pop the original value
            self.builder.emit(Opcode::Pop);
            return Ok(());
        }
        // Non-variable pattern - just pop for now
        self.builder.emit(Opcode::Pop);
        Ok(())
    }

    /// Compile a case expression using jump-based inline dispatch.
    ///
    /// MeTTa syntax: `(case scrutinee ((pattern1 body1) (pattern2 body2) ...))`
    ///
    /// Each arm emits:
    /// ```text
    /// Dup                       ; copy scrutinee for matching
    /// PushConstant(pattern)     ; push quoted pattern (preserves $vars)
    /// MatchBind                 ; pop pattern+copy, push bool, set bindings on match
    /// JumpIfFalse → next_arm    ; skip on mismatch (pops bool)
    /// Pop                       ; remove scrutinee (match succeeded)
    /// <compile body>            ; in tail position if case is in tail position
    /// Jump → end                ; skip remaining arms
    /// next_arm:
    /// ```
    /// After the last arm, a no-match fallback emits `Pop; PushEmpty`.
    fn compile_case(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() < 2 {
            return Err(CompileError::InvalidArity {
                op: "case".to_string(),
                expected: 2,
                got: args.len(),
            });
        }

        let scrutinee = &args[0];
        let branches = &args[1];

        // Extract (pattern, body) pairs from the branches S-expression
        let pairs: Vec<(&V, &V)> = if let Some(items) = branches.as_sexpr() {
            let mut pairs = Vec::with_capacity(items.len());
            for item in items {
                if let Some(pair) = item.as_sexpr() {
                    if pair.len() == 2 {
                        pairs.push((&pair[0], &pair[1]));
                    } else {
                        // Non-pair branch — fall back to EvalCase for safety
                        return self.compile_case_fallback(args);
                    }
                } else {
                    // Non-S-expression branch — fall back to EvalCase
                    return self.compile_case_fallback(args);
                }
            }
            pairs
        } else {
            // Branches is not an S-expression — fall back to EvalCase
            return self.compile_case_fallback(args);
        };

        // Install case barrier to catch failing scrutinees. If the scrutinee
        // produces zero results (Fail), the barrier redirects execution to
        // push the `Empty` atom, matching MeTTa HE case semantics.
        let barrier_jump = self.builder.emit_jump(Opcode::CaseBarrierBegin);

        // Compile scrutinee (not in tail position — it's an input)
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(scrutinee)?;
        self.in_tail_position = saved_tail;

        // Scrutinee succeeded: remove barrier
        self.builder.emit(Opcode::CaseBarrierEnd);
        let skip_handler = self.builder.emit_jump(Opcode::Jump);

        // Handler: push Empty atom (reached when scrutinee fails)
        self.builder.patch_jump(barrier_jump);
        let empty_atom = self.factory.atom("Empty");
        let empty_idx = self.builder.add_constant(empty_atom);
        self.builder.emit_u16(Opcode::PushConstant, empty_idx);

        self.builder.patch_jump(skip_handler);

        if pairs.is_empty() {
            // No arms: pop scrutinee, push empty
            self.builder.emit(Opcode::Pop);
            self.builder.emit(Opcode::PushEmpty);
            return Ok(());
        }

        let mut end_jumps: Vec<JumpLabel> = Vec::with_capacity(pairs.len());

        for (i, (pattern, body)) in pairs.iter().enumerate() {
            let is_last_arm = i == pairs.len() - 1;
            let is_catch_all = self.is_catch_all_pattern(pattern);

            if is_catch_all && is_last_arm {
                // Last arm with catch-all: no need for JumpIfFalse
                self.context.begin_scope();
                if pattern.is_variable() {
                    // Bind the variable as a compile-time local so the body
                    // can reference it via LoadLocal (not PushVariable).
                    self.bind_pattern(pattern)?;
                } else {
                    // Wildcard or other catch-all: just pop scrutinee
                    self.builder.emit(Opcode::Pop);
                }
                self.in_tail_position = saved_tail;
                self.compile(body)?;
                // Bug-Fix Phase 2b (2026-04): no cleanup emission — see compile_let.
                let _local_count = self.context.end_scope();
            } else {
                // Standard arm: isolate MatchBind variables to this arm.
                // The frame must exist before quoting the pattern so outer
                // bindings remain visible while prior arm bindings do not leak.
                self.builder.emit(Opcode::PushBindingFrame);

                // Dup scrutinee, push pattern, swap to MatchBind's [pattern,
                // value] stack contract.
                self.builder.emit(Opcode::Dup);
                self.compile_quoted(pattern)?;
                self.builder.emit(Opcode::Swap);
                self.builder.emit(Opcode::MatchBind);
                let next_arm = self.builder.emit_jump(Opcode::JumpIfFalse);

                // Match succeeded: bind pattern variables as locals so the body
                // can reference them via LoadLocal. Then compile body.
                self.context.begin_scope();
                if pattern.is_variable() {
                    // Scrutinee is still on stack — store as local
                    self.bind_pattern(pattern)?;
                } else {
                    self.builder.emit(Opcode::Pop); // pop scrutinee
                }
                self.in_tail_position = saved_tail;
                self.compile(body)?;
                // Bug-Fix Phase 2b (2026-04): no cleanup emission — see compile_let.
                let _local_count = self.context.end_scope();

                self.builder.emit(Opcode::PopBindingFrame);

                // Jump to end on success. This is required even for the last
                // non-catch-all arm, otherwise a matched arm falls through into
                // the no-match Fail fallback.
                let end_jump = self.builder.emit_jump(Opcode::Jump);
                end_jumps.push(end_jump);

                // Patch JumpIfFalse to here (next arm or fallback)
                self.builder.patch_jump(next_arm);
                self.builder.emit(Opcode::PopBindingFrame);
            }
        }

        // If the last arm was NOT a catch-all, emit no-match fallback.
        //
        // **MeTTa HE semantic note**: when no case arm matches the scrutinee,
        // the case expression must produce ZERO results (empty multiset),
        // matching MeTTa HE's `case`/`switch-minimal` behavior. The previous
        // implementation emitted `Pop; PushEmpty` which produces ONE Unit
        // result (`()`), causing PLN's deriver task queue to be polluted
        // with stray Unit values when the deriver's `(case (|- $x) ...)`
        // had no matching arm.
        //
        // The fix is `Pop; Fail`: Pop discards the unmatched scrutinee, and
        // Fail triggers backtracking. Because we are now PAST the
        // CaseBarrierEnd (line ~1397), the Fail propagates OUT of the case
        // expression to the enclosing context (rather than being caught by
        // the case barrier's scrutinee-failure handler). With no choice
        // points to backtrack to, the entire case sub-eval produces zero
        // results — matching MeTTa HE.
        let last_is_catch_all = self.is_catch_all_pattern(pairs.last().expect("non-empty").0);
        if !last_is_catch_all {
            self.builder.emit(Opcode::Pop);
            self.builder.emit(Opcode::Fail);
        }

        // Patch all end jumps to here
        for jump in end_jumps {
            self.builder.patch_jump(jump);
        }

        Ok(())
    }

    /// Fallback: compile case using EvalCase opcode (for malformed branches).
    fn compile_case_fallback(&mut self, args: &[V]) -> CompileResult<()> {
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(&args[0])?;
        self.in_tail_position = saved_tail;
        let case_branches = self.factory.sexpr(args[1..].to_vec());
        let case_idx = self.builder.add_constant(case_branches);
        self.builder.emit_u16(Opcode::EvalCase, case_idx);
        Ok(())
    }

    /// Check if a pattern is a catch-all (always matches).
    /// Catch-all patterns: wildcard `_`, bare variable `$x`, `&var`, `'var`.
    fn is_catch_all_pattern(&self, pattern: &V) -> bool {
        if let Some(name) = pattern.as_atom() {
            name == "_"
                || name.starts_with('$')
                || name.starts_with('\'')
                || (name.starts_with('&')
                    && name != "&"
                    && name != "&self"
                    && name != "&kb"
                    && name != "&stack")
        } else {
            false
        }
    }

    /// Compile a quoted expression: structural literal where `$`-prefixed
    /// atoms RESOLVE through the active binding chain (locals → upvalues →
    /// runtime binding frame) but S-expr heads are NOT dispatched as
    /// function calls.
    ///
    /// This brings the bytecode VM into bisimilarity with the trampoline's
    /// `apply_bindings_with_rename_scoped` semantics: by the time a quoted
    /// form reaches `SpaceAdd` / `SpaceRemove` / `MatchBind` / `Unify4` /
    /// `DefineRule`, all outer-scope variables have been substituted into
    /// the structural data, while inner pattern variables (unbound under
    /// the current frame) fall through `op_push_variable` to atom literals.
    ///
    /// Functionally equivalent to (and delegates to) `compile_as_literal_sexpr`
    /// — both names are kept for caller-side intent expression: this method
    /// names the "quoted form" semantic context (`add-atom`, `remove-atom`,
    /// `=`, `quote`, `case` patterns, 4-arg `unify` pattern2), while
    /// `compile_as_literal_sexpr` names the structural-walk mechanism.
    fn compile_quoted(&mut self, expr: &V) -> CompileResult<()> {
        self.compile_as_literal_sexpr(expr)
    }

    /// Compile superpose (nondeterminism)
    fn compile_superpose(&mut self, args: &[V]) -> CompileResult<()> {
        self.check_arity("superpose", args.len(), 1)?;

        let list = &args[0];
        // Unit is the normalized form of SExpr([]) - treat as empty superpose
        if list.is_unit() {
            if self.in_collapse_scope {
                // Inside collapse: push Unit so CollapseEnd collects nothing
                // (it filters Unit values). Fail would escape the collapse barrier.
                self.builder.emit(Opcode::PushUnit);
            } else {
                self.builder.emit(Opcode::Fail);
            }
            return Ok(());
        }
        if let Some(items) = list.as_sexpr() {
            if items.is_empty() {
                if self.in_collapse_scope {
                    self.builder.emit(Opcode::PushUnit);
                } else {
                    self.builder.emit(Opcode::Fail);
                }
                return Ok(());
            }

            if items.len() == 1 {
                // Single item - just compile it
                return self.compile(&items[0]);
            }

            // Multiple items: branch to inline compiled alternatives. Keeping
            // alternatives in the parent chunk preserves local slots and binding
            // frames, and lets failing alternatives `(empty)` backtrack before a
            // surrounding collapse collects anything.
            let count = items.len() as u16;
            self.builder.emit_u16(Opcode::ForkInline, count);
            let mut target_operands = Vec::with_capacity(items.len());
            for _ in items {
                let operand_offset = self.builder.current_offset();
                self.builder.emit_raw(&0u16.to_be_bytes());
                target_operands.push(operand_offset);
            }

            let mut end_jumps = Vec::with_capacity(items.len());
            for (item, operand_offset) in items.iter().zip(target_operands.into_iter()) {
                let target = self.builder.current_offset();
                if target > u16::MAX as usize {
                    return Err(CompileError::InvalidExpression(
                        "superpose branch target exceeds u16 bytecode address".to_string(),
                    ));
                }
                self.builder.patch_u16_at(operand_offset, target as u16);
                self.compile(item)?;
                end_jumps.push(self.builder.emit_jump(Opcode::Jump));
            }

            for jump in end_jumps {
                self.builder.patch_jump(jump);
            }

            // Inside collapse scope: omit Yield. CollapseEnd drives backtracking
            // via op_fail_within_collapse. Outside collapse scope: Yield saves
            // each branch result and backtracks normally.
            if !self.in_collapse_scope {
                self.builder.emit(Opcode::Yield);
            }
        } else {
            // Not a literal list - evaluate it, then superpose the runtime list value.
            self.compile(list)?;
            self.builder.emit(Opcode::EvalSuperpose);
            if !self.in_collapse_scope {
                self.builder.emit(Opcode::Yield);
            }
        }

        Ok(())
    }

    /// Compile chain operation
    fn compile_chain(&mut self, args: &[V]) -> CompileResult<()> {
        if args.len() != 3 {
            return Err(CompileError::InvalidArity {
                op: "chain".to_string(),
                expected: 3,
                got: args.len(),
            });
        }

        let expr = &args[0];
        let var = &args[1];
        let body = &args[2];

        // Compile expression
        let saved_tail = self.in_tail_position;
        self.in_tail_position = false;
        self.compile(expr)?;
        self.in_tail_position = saved_tail;

        // Bind result to variable
        self.context.begin_scope();
        self.bind_pattern(var)?;

        // Compile body
        self.compile(body)?;

        // Bug-Fix Phase 2b (2026-04): no cleanup emission — see compile_let.
        let _local_count = self.context.end_scope();

        Ok(())
    }

    /// Check arity of an operation
    pub(crate) fn check_arity(&self, op: &str, got: usize, expected: usize) -> CompileResult<()> {
        if got != expected {
            Err(CompileError::InvalidArity {
                op: op.to_string(),
                expected,
                got,
            })
        } else {
            Ok(())
        }
    }

    /// Finish compilation and return the chunk
    pub fn finish(mut self) -> GenericBytecodeChunk<V> {
        let offset = self.builder.current_offset();
        if offset == 0 || !self.ends_with_terminator() {
            self.builder.emit(Opcode::Return);
        }

        self.builder.set_local_count(self.context.local_count());
        self.builder.set_upvalue_count(self.context.upvalue_count());

        self.builder.build()
    }

    /// Check if the last emitted instruction is a terminator
    fn ends_with_terminator(&self) -> bool {
        false // Safe default
    }

    /// Finish and wrap in Arc
    pub fn finish_arc(self) -> Arc<GenericBytecodeChunk<V>> {
        Arc::new(self.finish())
    }
}

// =============================================================================
// Public API Functions
// =============================================================================

/// Compile a generic value to bytecode
pub fn compile_generic<V, F>(
    name: &str,
    expr: &V,
    factory: F,
) -> CompileResult<GenericBytecodeChunk<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let mut compiler = GenericCompiler::new(name, factory);
    compiler.compile(expr)?;
    Ok(compiler.finish())
}

/// Compile a generic value to bytecode wrapped in Arc
pub fn compile_generic_arc<V, F>(
    name: &str,
    expr: &V,
    factory: F,
) -> CompileResult<Arc<GenericBytecodeChunk<V>>>
where
    V: MettaValueTrait + Clone + Send + Sync + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone,
{
    Ok(Arc::new(compile_generic(name, expr, factory)?))
}

// =============================================================================
// Arena-Specific Entry Points
// =============================================================================

use crate::backend::eval::trampoline::get_static_factory;
use crate::backend::models::{GcFactory, MettaValue};

/// Compile an MettaValue expression to bytecode (zero-conversion).
///
/// Uses the static arena factory from the thread-local context.
pub fn compile_bytecode(
    name: &str,
    expr: &MettaValue,
) -> CompileResult<GenericBytecodeChunk<MettaValue>> {
    let factory = get_static_factory();
    compile_generic(name, expr, factory)
}

/// Compile an MettaValue expression to bytecode wrapped in Arc (zero-conversion).
pub fn compile_bytecode_arc(
    name: &str,
    expr: &MettaValue,
) -> CompileResult<Arc<GenericBytecodeChunk<MettaValue>>> {
    let factory = get_static_factory();
    compile_generic_arc(name, expr, factory)
}

/// Type alias for arena bytecode compiler
pub type MettaCompiler = GenericCompiler<MettaValue, GcFactory>;
