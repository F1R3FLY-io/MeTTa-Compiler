use std::fmt;
use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};

use super::gc_allocator::GcFactory;
use super::MettaValue;
use crate::backend::environment::MettaEnvironment;

// ============================================================================
// MettaStateGcRoots — GC root provider for source/output vectors
// ============================================================================

/// Internal storage for MettaState's source and output vectors, registered
/// as a GC root provider so the garbage collector can trace values in these
/// vectors during quiescent-state collection.
///
/// Uses `parking_lot::Mutex` for thread-safe, non-poisoning access.
/// Lock ordering: always source before output to prevent deadlocks.
struct MettaStateGcRoots {
    source: Mutex<Vec<MettaValue>>,
    output: Mutex<Vec<MettaValue>>,
}


// ============================================================================
// MettaState
// ============================================================================

/// MeTTa computation session state.
///
/// Holds compiled source expressions, the evaluation environment (atom space),
/// and evaluation output. All value allocation goes through the global
/// `SlabAllocator` via `GcFactory`.
///
/// Source and output vectors are stored behind `Arc<Mutex<...>>` and
/// automatically registered as GC root providers. The garbage collector
/// can trace values in these vectors during quiescent-state collection.
///
/// # State Composition
/// - **Compiled state** (fresh from `compile`):
///   - `source`: S-expressions to evaluate
///   - `environment`: Empty atom space
///   - `output`: Empty (no evaluations yet)
///
/// - **Accumulated state** (built over multiple REPL iterations):
///   - `source`: Empty (already evaluated)
///   - `environment`: Accumulated atom space (MORK facts/rules)
///   - `output`: Accumulated evaluation results
///
/// # Usage Pattern
/// ```ignore
/// // Compile MeTTa source
/// let compiled_state = compile(source)?;
///
/// // Run against accumulated state
/// let new_accumulated = accumulated_state.run(&compiled_state)?;
/// ```
pub struct MettaState {
    /// GC-registered root provider for source/output vectors.
    /// Uses `Arc` so the root registry holds a `Weak` reference.
    gc_roots: Arc<MettaStateGcRoots>,
    /// The atom space (MORK fact database) containing rules and facts.
    pub environment: MettaEnvironment,
}

impl MettaState {
    /// Create a new empty MettaState.
    pub fn new() -> Self {
        Self::new_empty()
    }

    /// Create a fresh compiled state from parse results.
    pub fn new_compiled(source: Vec<MettaValue>) -> Self {
        Self::from_parts(source, MettaEnvironment::default(), Vec::new())
    }

    /// Create an empty accumulated state (for REPL initialization).
    pub fn new_empty() -> Self {
        Self::from_parts(Vec::new(), MettaEnvironment::default(), Vec::new())
    }

    /// Create an accumulated state from just an environment (no prior output).
    /// Convenience for callers that have a MettaEnvironment and need a MettaState.
    pub fn from_env(environment: MettaEnvironment) -> Self {
        Self::new_accumulated(environment, Vec::new())
    }

    /// Create an accumulated state with existing environment and output.
    pub fn new_accumulated(environment: MettaEnvironment, output: Vec<MettaValue>) -> Self {
        Self::from_parts(Vec::new(), environment, output)
    }

    /// Internal constructor: creates gc_roots Arc and registers with GC.
    pub(crate) fn from_parts(
        source: Vec<MettaValue>,
        environment: MettaEnvironment,
        output: Vec<MettaValue>,
    ) -> Self {
        let gc_roots = Arc::new(MettaStateGcRoots {
            source: Mutex::new(source),
            output: Mutex::new(output),
        });
        MettaState {
            gc_roots,
            environment,
        }
    }

    /// Get the factory for allocating values.
    ///
    /// Returns a `GcFactory` backed by the global `SlabAllocator`.
    #[inline]
    pub fn factory(&self) -> GcFactory {
        // Construct the concrete slab factory directly. `global_factory()` is now
        // feature-polymorphic (returns `ActiveFactory`), so it can no longer
        // satisfy this concrete `GcFactory` return type. Byte-identical to the
        // previous body in the default build.
        GcFactory::new(super::gc_allocator::global_allocator())
    }

    /// Lock and access the source expressions.
    ///
    /// Returns a `MutexGuard` that auto-derefs to `Vec<MettaValue>`.
    /// The lock is released when the guard is dropped.
    ///
    /// # Temporary Lifetime Pitfall
    ///
    /// **Do NOT** use `state.source()[i]` directly inside an `eval()` call:
    ///
    /// ```ignore
    /// // UNSAFE — MutexGuard temporary lives until the end of the `let` statement,
    /// // so the source mutex is held for the entire duration of eval(). This can
    /// // deadlock with the GC thread (ABBA: source mutex vs GC_IN_PROGRESS).
    /// let (results, _env) = eval(state.source()[0], new_env(), &state);
    /// ```
    ///
    /// Instead, extract the value in a separate statement so the guard is dropped
    /// before `eval()` is called:
    ///
    /// ```ignore
    /// // SAFE — MutexGuard dropped at the semicolon, before eval() runs.
    /// let expr = state.source()[0];
    /// let (results, _env) = eval(expr, new_env(), &state);
    /// ```
    ///
    /// Or use [`source_snapshot`](MettaState::source_snapshot) which returns an
    /// owned `Vec<MettaValue>`, eliminating the guard entirely.
    #[inline]
    pub fn source(&self) -> MutexGuard<'_, Vec<MettaValue>> {
        self.gc_roots.source.lock()
    }

    /// Return an owned snapshot of the source expressions.
    ///
    /// This is the **preferred** way to access source expressions before
    /// calling `eval()` — the mutex is locked, copied, and released in
    /// a single expression, so there is no risk of holding the guard
    /// across a long-running operation (which would deadlock with the GC).
    ///
    /// ```ignore
    /// let source_exprs = state.source_snapshot();
    /// for expr in source_exprs {
    ///     let (results, new_env) = eval(expr, env, &state);
    ///     // ...
    /// }
    /// ```
    #[inline]
    pub fn source_snapshot(&self) -> Vec<MettaValue> {
        self.gc_roots.source.lock().iter().copied().collect()
    }

    /// Lock and mutably access the source expressions.
    ///
    /// Returns a `MutexGuard` with `DerefMut` to `Vec<MettaValue>`.
    /// Interior mutability — does not require `&mut self`.
    #[inline]
    pub fn source_mut(&self) -> MutexGuard<'_, Vec<MettaValue>> {
        self.gc_roots.source.lock()
    }

    /// Lock and access the output values.
    ///
    /// Returns a `MutexGuard` that auto-derefs to `Vec<MettaValue>`.
    /// The lock is released when the guard is dropped.
    ///
    /// # Temporary Lifetime Pitfall
    ///
    /// Same as [`source`](MettaState::source) — do NOT use `state.output()[i]`
    /// directly inside an `eval()` call. See `source()` docs for details.
    #[inline]
    pub fn output(&self) -> MutexGuard<'_, Vec<MettaValue>> {
        self.gc_roots.output.lock()
    }

    /// CESK A4.3 — collect the driver's program control (C) roots, read by name.
    ///
    /// `source` (the not-yet-evaluated top-level directives) and `output` (the
    /// accumulated `!`-results) are the DRIVER's control: held by the eval /
    /// conformance / rholang loop ABOVE the trampoline, not in the machine's
    /// ⟨C,E,K⟩. They are genuine GC roots (a later directive must survive a
    /// mid-eval collection of the current one). This reads them by name — the
    /// exact bodies [`MettaStateGcRoots::collect_roots`] uses — so the
    /// machine-equivalence oracle can treat them as a KEPT driver-C channel
    /// (read via the `EvalContext` seam) instead of false-failing on them.
    /// Appends to `out` (never clears).
    ///
    /// A5.4 re-homes this into the narrowed `SAFEPOINT_ROOTS` publication buffer
    /// (the "driver's C" channel) when `ROOT_REGISTRY` is deleted.
    #[inline]
    pub fn collect_driver_program_roots(&self, out: &mut Vec<MettaValue>) {
        // Lock order source-before-output matches the RootProvider impl above
        // (no new lock-ordering edge). The driver releases these before calling
        // eval (the ABBA-deadlock fix), so the safepoint lock is uncontended in
        // the single-threaded index regime.
        let source = self.gc_roots.source.lock();
        let output = self.gc_roots.output.lock();
        out.extend(source.iter().copied());
        out.extend(output.iter().copied());
    }

    /// Return an owned snapshot of the output values.
    ///
    /// See [`source_snapshot`](MettaState::source_snapshot) for rationale.
    #[inline]
    pub fn output_snapshot(&self) -> Vec<MettaValue> {
        self.gc_roots.output.lock().iter().copied().collect()
    }

    /// Lock and mutably access the output values.
    ///
    /// Returns a `MutexGuard` with `DerefMut` to `Vec<MettaValue>`.
    /// Interior mutability — does not require `&mut self`.
    #[inline]
    pub fn output_mut(&self) -> MutexGuard<'_, Vec<MettaValue>> {
        self.gc_roots.output.lock()
    }

    /// Push a value to the output vector.
    ///
    /// Convenience method that locks output internally.
    #[inline]
    pub fn push_output(&self, value: MettaValue) {
        self.gc_roots.output.lock().push(value);
    }

    /// Clear output (useful for reusing MettaState across evaluations).
    #[inline]
    pub fn clear_output(&self) {
        self.gc_roots.output.lock().clear();
    }

    /// Convert MettaState to JSON representation for debugging
    ///
    /// Returns a JSON string with the format:
    /// ```json
    /// {
    ///   "source": [...],
    ///   "environment": {"facts_count": N},
    ///   "output": [...]
    /// }
    /// ```
    ///
    /// **Use Case**: Debugging, logging, inspection
    /// **Not Recommended**: Rholang integration (use PathMap Par instead)
    pub fn to_json_string(&self) -> String {
        // Snapshot under lock, then release before formatting.
        // Prevents holding both Mutexes during potentially slow formatting.
        let (source_snapshot, output_snapshot) = {
            let source = self.gc_roots.source.lock();
            let output = self.gc_roots.output.lock();
            (source.clone(), output.clone())
        };

        let source_json: Vec<String> = source_snapshot
            .iter()
            .map(|value| value.to_json_string())
            .collect();

        let outputs_json: Vec<String> = output_snapshot
            .iter()
            .map(|value| value.to_json_string())
            .collect();

        // For environment, we'll serialize facts count as a placeholder
        // Full serialization of MORK Space would require more complex handling
        let env_json = format!(r#"{{"facts_count":{}}}"#, self.environment.rule_count());

        format!(
            r#"{{"source":[{}],"environment":{},"output":[{}]}}"#,
            source_json.join(","),
            env_json,
            outputs_json.join(",")
        )
    }
}

impl Default for MettaState {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MettaState {
    /// Deep-clone the MettaState, creating a new `Arc<MettaStateGcRoots>` and
    /// registering it as a separate GC root provider.
    fn clone(&self) -> Self {
        // Snapshot under lock, then release before from_parts() which does
        // GC registration (register_root_provider). This prevents holding
        // both Mutexes during the Arc allocation and GC registration.
        let (source_clone, output_clone) = {
            let source = self.gc_roots.source.lock();
            let output = self.gc_roots.output.lock();
            (source.clone(), output.clone())
        };
        Self::from_parts(source_clone, self.environment.clone(), output_clone)
    }
}

impl fmt::Debug for MettaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Snapshot under lock, then release before formatting.
        // Prevents holding both Mutexes during potentially slow Debug output.
        let (source_snapshot, output_snapshot) = {
            let source = self.gc_roots.source.lock();
            let output = self.gc_roots.output.lock();
            (source.clone(), output.clone())
        };
        f.debug_struct("MettaState")
            .field("source", &source_snapshot)
            .field("environment", &self.environment)
            .field("output", &output_snapshot)
            .finish()
    }
}

impl From<MettaValue> for MettaState {
    /// Create a compiled state containing an error s-expression.
    /// Used when parsing fails to allow error handling at the evaluation level.
    fn from(error_sexpr: MettaValue) -> Self {
        Self::from_parts(vec![error_sexpr], MettaEnvironment::default(), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile;
    // Rule type removed — rules are (lhs, rhs) tuples stored as (= lhs rhs) in PathMap

    #[test]
    fn test_to_json_empty() {
        let state = MettaState::new_empty();
        let json = state.to_json_string();

        // Should have empty arrays for source and output, and facts_count 0
        assert_eq!(
            json,
            r#"{"source":[],"environment":{"facts_count":0},"output":[]}"#
        );
    }

    #[test]
    fn test_to_json_with_source() {
        let state = MettaState::new_compiled(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(42),
        ]);
        let json = state.to_json_string();

        // Should contain source values
        assert!(json.contains(r#""source":["#));
        assert!(json.contains(r#"{"type":"atom","value":"test"}"#));
        assert!(json.contains(r#"{"type":"number","value":42}"#));
        assert!(json.contains(r#""environment":{"facts_count":0}"#));
        assert!(json.contains(r#""output":[]"#));
    }

    #[test]
    fn test_to_json_with_output() {
        let state = MettaState::new_accumulated(
            MettaEnvironment::default(),
            vec![
                MettaValue::Bool(true),
                MettaValue::String("result".to_string()),
            ],
        );
        let json = state.to_json_string();

        // Should contain output values
        assert!(json.contains(r#""source":[]"#));
        assert!(json.contains(r#""environment":{"facts_count":0}"#));
        assert!(json.contains(r#""output":["#));
        assert!(json.contains(r#"{"type":"bool","value":true}"#));
        assert!(json.contains(r#"{"type":"string","value":"result"}"#));
    }

    #[test]
    fn test_to_json_with_environment() {
        let mut env = MettaEnvironment::default();
        env.add_rule(MettaValue::Atom("x".to_string()), MettaValue::Long(1));
        env.add_rule(MettaValue::Atom("y".to_string()), MettaValue::Long(2));

        let state = MettaState::new_accumulated(env, Vec::new());
        let json = state.to_json_string();

        // Should show facts_count as 2
        assert!(json.contains(r#""environment":{"facts_count":2}"#));
    }

    #[test]
    fn test_to_json_complete() {
        let mut env = MettaEnvironment::default();
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        );

        let state = MettaState::from_parts(
            vec![MettaValue::Atom("test".to_string())],
            env,
            vec![MettaValue::Long(10)],
        );

        let json = state.to_json_string();

        // Should contain all fields
        assert!(json.contains(r#""source":["#));
        assert!(json.contains(r#"{"type":"atom","value":"test"}"#));
        assert!(json.contains(r#""environment":{"facts_count":1}"#));
        assert!(json.contains(r#""output":["#));
        assert!(json.contains(r#"{"type":"number","value":10}"#));
    }

    #[test]
    fn test_to_json_sexpr_values() {
        let state = MettaState::from_parts(
            vec![MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ])],
            MettaEnvironment::default(),
            vec![MettaValue::SExpr(vec![
                MettaValue::Atom("result".to_string()),
                MettaValue::Long(3),
            ])],
        );

        let json = state.to_json_string();

        // Should properly serialize s-expressions
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""items""#));
    }

    #[test]
    fn test_to_json() {
        let src = "(+ 1 2)";
        let state = compile(src).unwrap();
        let json = state.to_json_string();

        // Should return full MettaState with source, environment, output
        assert!(json.contains(r#""source""#));
        assert!(json.contains(r#""environment""#));
        assert!(json.contains(r#""output""#));
        assert!(json.contains(r#""type":"sexpr""#));
    }

    #[test]
    fn test_clone_creates_independent_roots() {
        let state = MettaState::new_compiled(vec![MettaValue::Atom("test".to_string())]);

        let cloned = state.clone();

        // Cloned state should have same source content
        assert_eq!(cloned.source().len(), 1);

        // But modifying the clone shouldn't affect the original
        cloned.source_mut().push(MettaValue::Long(42));
        assert_eq!(state.source().len(), 1);
        assert_eq!(cloned.source().len(), 2);
    }

    #[test]
    fn test_push_output_convenience() {
        let state = MettaState::new_empty();
        state.push_output(MettaValue::Long(1));
        state.push_output(MettaValue::Long(2));
        assert_eq!(state.output().len(), 2);
    }

    #[test]
    fn test_clear_output() {
        let state =
            MettaState::new_accumulated(MettaEnvironment::default(), vec![MettaValue::Long(1)]);
        assert_eq!(state.output().len(), 1);
        state.clear_output();
        assert_eq!(state.output().len(), 0);
    }

    #[test]
    fn test_interior_mutability() {
        // source_mut() and output_mut() should work without &mut self
        let state = MettaState::new_empty();
        state
            .source_mut()
            .push(MettaValue::Atom("test".to_string()));
        state.output_mut().push(MettaValue::Long(42));
        assert_eq!(state.source().len(), 1);
        assert_eq!(state.output().len(), 1);
    }

    #[test]
    fn test_from_error_sexpr() {
        let error = MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::String("test error".to_string()),
        ]);
        let state = MettaState::from(error);
        assert_eq!(state.source().len(), 1);
        assert_eq!(state.output().len(), 0);
    }
}
