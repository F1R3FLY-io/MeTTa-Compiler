//! Conversion utilities between MettaValue and MORK Expr format
//!
//! This module handles the bidirectional conversion needed for query_multi integration:
//! - MettaValue → MORK Expr (for pattern queries)
//! - MORK bindings → SmallVec<[(String, MettaValue); 8]> (for pattern match results)
//!
//! ## Optimization: Buffer Pooling
//!
//! Instead of allocating a new 256KB buffer for every `metta_to_mork_bytes` call,
//! we maintain a thread-local pool of reusable buffers. This eliminates:
//! - ~4% overhead from malloc/free for 256KB buffers
//! - Arena fragmentation from frequent large allocations
//!
//! ## Optimization: Context Pooling
//!
//! The `ConversionContext` (containing HashMap and Vec) is now pooled to avoid
//! allocation overhead for these internal structures.

use super::models::{Bindings, MettaValue, MettaValueInner, MettaValueTrait};
use mork::space::{ParDataParser, Space};
use mork_expr::{Expr, ExprEnv, ExprZipper};
use mork_frontend::bytestring_parser::Parser;
use std::cell::RefCell;
use std::collections::HashMap;
use tracing::{debug, trace, warn};

// ============================================================================
// Buffer Pool - Reuse 256KB buffers across calls
// ============================================================================

/// Thread-local buffer pool for MORK conversion.
/// Avoids repeated 256KB allocations.
thread_local! {
    static BUFFER_POOL: RefCell<BufferPool> = RefCell::new(BufferPool::new());
    static CONTEXT_POOL: RefCell<Vec<ConversionContext>> = RefCell::new(Vec::new());
}

/// Pool of reusable byte buffers for MORK conversion.
struct BufferPool {
    /// Available buffers ready for reuse
    buffers: Vec<Vec<u8>>,
    /// Size hint for new buffers based on recent usage
    size_hint: usize,
}

impl BufferPool {
    /// Create a new empty buffer pool.
    fn new() -> Self {
        BufferPool {
            buffers: Vec::new(),
            // Start with 4KB, grow based on usage
            size_hint: 4096,
        }
    }

    /// Acquire a buffer from the pool or create a new one.
    fn acquire(&mut self) -> Vec<u8> {
        if let Some(mut buffer) = self.buffers.pop() {
            // Clear the buffer for reuse
            buffer.clear();
            // Ensure capacity meets current size hint
            if buffer.capacity() < self.size_hint {
                buffer.reserve(self.size_hint - buffer.capacity());
            }
            buffer
        } else {
            // No buffer available, create new one
            // Use max of size_hint and minimum 4KB
            let capacity = self.size_hint.max(4096);
            vec![0u8; capacity]
        }
    }

    /// Return a buffer to the pool for reuse.
    fn release(&mut self, buffer: Vec<u8>) {
        // Update size hint based on actual usage
        // This allows the pool to adapt to workload patterns
        if buffer.len() > self.size_hint {
            // Double the size hint to reduce reallocations, capped at 256KB
            self.size_hint = (buffer.len() * 2).min(262144);
        }

        // Keep up to 4 buffers in the pool
        if self.buffers.len() < 4 {
            self.buffers.push(buffer);
        }
        // Otherwise let the buffer drop
    }
}

/// RAII guard that returns buffer to pool on drop.
pub struct PooledBuffer {
    buffer: Option<Vec<u8>>,
}

impl PooledBuffer {
    /// Get a buffer from the thread-local pool.
    pub fn acquire() -> Self {
        let buffer = BUFFER_POOL.with(|pool| pool.borrow_mut().acquire());
        PooledBuffer {
            buffer: Some(buffer),
        }
    }

    /// Get mutable access to the buffer.
    pub fn as_mut(&mut self) -> &mut Vec<u8> {
        self.buffer.as_mut().expect("buffer already released")
    }

    /// Get the buffer's slice.
    pub fn as_slice(&self) -> &[u8] {
        self.buffer.as_ref().expect("buffer already released")
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            BUFFER_POOL.with(|pool| pool.borrow_mut().release(buffer));
        }
    }
}

// ============================================================================
// Context Pooling - Reuse ConversionContext across calls
// ============================================================================

/// Acquire a ConversionContext from the pool or create a new one.
pub fn acquire_context() -> ConversionContext {
    CONTEXT_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(ConversionContext::new)
    })
}

/// Return a ConversionContext to the pool for reuse.
pub fn release_context(mut ctx: ConversionContext) {
    // Reset for reuse
    ctx.var_map.clear();
    ctx.var_names.clear();

    CONTEXT_POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        // Keep up to 4 contexts
        if pool.len() < 4 {
            pool.push(ctx);
        }
    });
}

/// Context for tracking variables during MettaValue → Expr conversion
#[derive(Default)]
pub struct ConversionContext {
    /// Maps variable names to their De Bruijn indices
    pub var_map: HashMap<String, u8>,
    /// Reverse map: De Bruijn index → variable name
    pub var_names: Vec<String>,
}

impl ConversionContext {
    pub fn new() -> Self {
        ConversionContext {
            var_map: HashMap::new(),
            var_names: Vec::new(),
        }
    }

    /// Get or create a De Bruijn index for a variable
    pub fn get_or_create_var(&mut self, name: &str) -> Result<Option<u8>, String> {
        if let Some(&idx) = self.var_map.get(name) {
            // Variable already exists, return its index
            Ok(Some(idx))
        } else {
            // New variable
            if self.var_names.len() >= 64 {
                return Err("Too many variables (max 64)".to_string());
            }
            let idx = self.var_names.len() as u8;
            self.var_map.insert(name.to_string(), idx);
            self.var_names.push(name.to_string());
            Ok(None) // None means "write NewVar tag"
        }
    }
}

/// Convert MettaValue to MORK Expr bytes
///
/// This creates a MORK s-expression that can be used with query_multi.
/// Variables are converted to De Bruijn indices.
///
/// ## Optimization: Buffer Pooling
///
/// Instead of allocating a new 256KB buffer for each call, we use a thread-local
/// buffer pool. This eliminates ~4% malloc/free overhead for large expressions.
pub fn metta_to_mork_bytes<V: Clone + Default + Send + Sync + Unpin>(
    value: &MettaValue,
    space: &Space<V>,
    ctx: &mut ConversionContext,
) -> Result<Vec<u8>, String> {
    trace!(
        target: "mettatron::conversion::metta_to_mork_bytes",
        ?value, "Converting MettaValue to MORK bytes"
    );

    // Use pooled buffer instead of allocating fresh 256KB each time
    let mut pooled = PooledBuffer::acquire();
    let buffer = pooled.as_mut();

    // Ensure buffer has enough capacity (grow if needed)
    // Most expressions are small, but mmverify can have complex nested structures
    const MAX_BUFFER_SIZE: usize = 262144;
    if buffer.len() < MAX_BUFFER_SIZE {
        buffer.resize(MAX_BUFFER_SIZE, 0);
    }

    let expr = Expr {
        ptr: buffer.as_mut_ptr(),
    };
    let mut ez = ExprZipper::new(expr);

    // Create ParDataParser once for the entire conversion to avoid data races.
    // MORK's threading model assumes each thread holds ONE WritePermit for the duration
    // of operations. Creating a new ParDataParser per symbol (as was done before) violated
    // this assumption and caused races when multiple threads accessed the same Slab chain.
    let mut pdp = ParDataParser::new(&space.sm);

    write_metta_value(value, &mut pdp, ctx, &mut ez).map_err(|e| {
        debug!(
            target: "mettatron::conversion::metta_to_mork_bytes",
            error = %e, "Conversion to MORK bytes failed"
        );
        e
    })?;

    // Safety check: ensure we didn't overflow the buffer
    if ez.loc > MAX_BUFFER_SIZE {
        return Err(format!(
            "Expression too large for MORK conversion: {} bytes (max {})",
            ez.loc, MAX_BUFFER_SIZE
        ));
    }

    // Copy result to a new Vec (buffer returns to pool on drop)
    Ok(buffer[..ez.loc].to_vec())
}

/// Convert MettaValue to MORK Expr bytes using a pooled context.
///
/// This is a convenience function that acquires and releases the context automatically.
/// Use this when you don't need to access the variable mappings after conversion.
pub fn metta_to_mork_bytes_pooled<V: Clone + Default + Send + Sync + Unpin>(
    value: &MettaValue,
    space: &Space<V>,
) -> Result<Vec<u8>, String> {
    let mut ctx = acquire_context();
    let result = metta_to_mork_bytes(value, space, &mut ctx);
    release_context(ctx);
    result
}

/// Recursively write MettaValue to ExprZipper
fn write_metta_value(
    value: &MettaValue,
    pdp: &mut ParDataParser,
    ctx: &mut ConversionContext,
    ez: &mut ExprZipper,
) -> Result<(), String> {
    match value.inner() {
        MettaValueInner::Atom(name) => {
            // Check if it's a variable
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            // EXCEPT: "&self", "&kb", "&stack" are space references, not variables
            if *name == "&" || *name == "&self" || *name == "&kb" || *name == "&stack" {
                // Space references and standalone & are NOT variables - write as symbols
                write_symbol(name.as_bytes(), pdp, ez)?;
            } else if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') {
                // Variable - use De Bruijn encoding
                let var_id = &name[1..]; // Remove prefix
                match ctx.get_or_create_var(var_id)? {
                    None => {
                        // First occurrence - write NewVar
                        ez.write_new_var();
                        ez.loc += 1;
                    }
                    Some(idx) => {
                        // Subsequent occurrence - write VarRef
                        ez.write_var_ref(idx);
                        ez.loc += 1;
                    }
                }
            } else if *name == "_" {
                // Wildcard - treat as anonymous variable
                ez.write_new_var();
                ez.loc += 1;
            } else {
                // Regular atom - write as symbol
                write_symbol(name.as_bytes(), pdp, ez)?;
            }
        }

        MettaValueInner::Bool(b) => {
            let s = if *b { "true" } else { "false" };
            write_symbol(s.as_bytes(), pdp, ez)?;
        }

        MettaValueInner::Long(n) => {
            let s = n.to_string();
            write_symbol(s.as_bytes(), pdp, ez)?;
        }

        MettaValueInner::Float(f) => {
            let s = f.to_string();
            write_symbol(s.as_bytes(), pdp, ez)?;
        }

        MettaValueInner::String(s) => {
            // MORK uses quoted strings
            let quoted = format!("\"{}\"", s);
            write_symbol(quoted.as_bytes(), pdp, ez)?;
        }

        MettaValueInner::Unit => {
            // Empty list
            ez.write_arity(0);
            ez.loc += 1;
        }

        MettaValueInner::SExpr(items) => {
            // MORK arity is limited to 6 bits (0-63)
            if items.len() >= 64 {
                return Err(format!(
                    "Expression has too many children ({}) - MORK arity limit is 63",
                    items.len()
                ));
            }
            // Write arity tag
            let arity = items.len() as u8;
            ez.write_arity(arity);
            ez.loc += 1;

            // Write each element
            for item in *items {
                write_metta_value(item, pdp, ctx, ez)?;
            }
        }

        MettaValueInner::Error(msg, details) => {
            // (error "msg" details)
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"error", pdp, ez)?;
            write_symbol(format!("\"{}\"", msg).as_bytes(), pdp, ez)?;
            write_metta_value(details, pdp, ctx, ez)?;
        }

        MettaValueInner::Type(t) => {
            // Types are just atoms/expressions
            write_metta_value(t, pdp, ctx, ez)?;
        }

        MettaValueInner::Conjunction(goals) => {
            // MORK arity is limited to 6 bits (0-63)
            // +1 for the comma symbol
            let total_arity = goals.len() + 1;
            if total_arity >= 64 {
                return Err(format!(
                    "Conjunction has too many goals ({}) - MORK arity limit is 63",
                    goals.len()
                ));
            }
            // Conjunctions are written as (, goal1 goal2 ...) with comma as first symbol
            ez.write_arity(total_arity as u8);
            ez.loc += 1;

            // Write the comma symbol as first child
            write_symbol(b",", pdp, ez)?;

            // Write each goal
            for goal in *goals {
                write_metta_value(goal, pdp, ctx, ez)?;
            }
        }

        // Space references are written as (Space id name)
        MettaValueInner::Space(handle) => {
            ez.write_arity(3);
            ez.loc += 1;
            write_symbol(b"Space", pdp, ez)?;
            write_symbol(handle.id.to_string().as_bytes(), pdp, ez)?;
            write_symbol(format!("\"{}\"", handle.name).as_bytes(), pdp, ez)?;
        }

        // State references are written as (State id)
        MettaValueInner::State(id) => {
            ez.write_arity(2);
            ez.loc += 1;
            write_symbol(b"State", pdp, ez)?;
            write_symbol(id.to_string().as_bytes(), pdp, ez)?;
        }

        // Memo tables are runtime-only and cannot be stored in MORK
        MettaValueInner::Memo(handle) => {
            return Err(format!(
                "Cannot convert Memo table '{}' (id={}) to MORK - memoization tables are runtime-only",
                handle.name, handle.id
            ));
        }

        // Empty sentinel is runtime-only and should be filtered out before MORK conversion
        MettaValueInner::Empty => {
            return Err(
                "Cannot convert Empty sentinel to MORK - Empty should be filtered at result collection".to_string()
            );
        }
    }

    Ok(())
}

/// Write a symbol to ExprZipper using the provided ParDataParser
///
/// The caller must provide a ParDataParser (which holds a WritePermit) that is held
/// for the duration of the entire conversion operation. This ensures MORK's threading
/// model is respected - each thread holds ONE WritePermit, not one per symbol.
fn write_symbol(bytes: &[u8], pdp: &mut ParDataParser, ez: &mut ExprZipper) -> Result<(), String> {
    let token = pdp.tokenizer(bytes);
    ez.write_symbol(token);
    ez.loc += 1 + token.len();
    Ok(())
}

// ============================================================================
// Generic MORK Conversion - Zero-Conversion for MettaValue
// ============================================================================

/// Convert any MettaValueTrait value to MORK Expr bytes (GENERIC VERSION).
///
/// This function uses `MettaValueTrait` methods instead of `MettaValueInner` pattern
/// matching, enabling zero-conversion for MettaValue ↔ MORK operations.
///
/// ## Zero-Conversion Path
///
/// For MettaValue:
/// ```text
/// MettaValue → value_to_mork_bytes_generic() → MORK bytes → PathMap
/// PathMap → MORK bytes → mork_expr_to_metta_value() → MettaValue
/// ```
///
/// No MettaValue involved, no heap allocations for transient values.
///
/// ## Performance
///
/// Uses the same optimizations as `metta_to_mork_bytes`:
/// - Buffer pooling (thread-local reusable buffers)
/// - Context pooling (reusable variable tracking)
/// - Single ParDataParser per conversion (proper MORK threading)
pub fn value_to_mork_bytes_generic<V, M>(
    value: &V,
    space: &Space<M>,
    ctx: &mut ConversionContext,
) -> Result<Vec<u8>, String>
where
    V: MettaValueTrait,
    M: Clone + Default + Send + Sync + Unpin,
{
    trace!(
        target: "mettatron::conversion::value_to_mork_bytes_generic",
        "Converting generic value to MORK bytes"
    );

    // Use pooled buffer instead of allocating fresh 256KB each time
    let mut pooled = PooledBuffer::acquire();
    let buffer = pooled.as_mut();

    // Ensure buffer has enough capacity (grow if needed)
    const MAX_BUFFER_SIZE: usize = 262144;
    if buffer.len() < MAX_BUFFER_SIZE {
        buffer.resize(MAX_BUFFER_SIZE, 0);
    }

    let expr = Expr {
        ptr: buffer.as_mut_ptr(),
    };
    let mut ez = ExprZipper::new(expr);

    // Create ParDataParser once for the entire conversion
    let mut pdp = ParDataParser::new(&space.sm);

    write_value_generic(value, &mut pdp, ctx, &mut ez).map_err(|e| {
        debug!(
            target: "mettatron::conversion::value_to_mork_bytes_generic",
            error = %e, "Generic conversion to MORK bytes failed"
        );
        e
    })?;

    // Safety check
    if ez.loc > MAX_BUFFER_SIZE {
        return Err(format!(
            "Expression too large for MORK conversion: {} bytes (max {})",
            ez.loc, MAX_BUFFER_SIZE
        ));
    }

    // Copy result to a new Vec (buffer returns to pool on drop)
    Ok(buffer[..ez.loc].to_vec())
}

/// Convenience wrapper for value_to_mork_bytes_generic with pooled context.
pub fn value_to_mork_bytes_generic_pooled<V, M>(
    value: &V,
    space: &Space<M>,
) -> Result<Vec<u8>, String>
where
    V: MettaValueTrait,
    M: Clone + Default + Send + Sync + Unpin,
{
    let mut ctx = acquire_context();
    let result = value_to_mork_bytes_generic(value, space, &mut ctx);
    release_context(ctx);
    result
}

/// Generic recursive writer using MettaValueTrait methods (stack-based to avoid recursion).
///
/// Uses trait accessors (`as_atom()`, `as_sexpr()`, etc.) instead of `MettaValueInner`
/// pattern matching. This enables the same code to work with both MettaValue and MettaValue.
fn write_value_generic<V: MettaValueTrait>(
    value: &V,
    pdp: &mut ParDataParser,
    ctx: &mut ConversionContext,
    ez: &mut ExprZipper,
) -> Result<(), String> {
    // Stack-based traversal to handle deep S-expressions without stack overflow
    enum WorkItem<'a, V: MettaValueTrait> {
        Process(&'a V),
    }

    let mut work_stack: Vec<WorkItem<V>> = vec![WorkItem::Process(value)];

    while let Some(item) = work_stack.pop() {
        match item {
            WorkItem::Process(v) => {
                // Atoms (including variables)
                if let Some(name) = v.as_atom() {
                    // Check special atoms that are NOT variables
                    if name == "&" || name == "&self" || name == "&kb" || name == "&stack" {
                        write_symbol(name.as_bytes(), pdp, ez)?;
                    } else if name.starts_with('$')
                        || name.starts_with('&')
                        || name.starts_with('\'')
                    {
                        // Variable - use De Bruijn encoding
                        let var_id = &name[1..];
                        match ctx.get_or_create_var(var_id)? {
                            None => {
                                ez.write_new_var();
                                ez.loc += 1;
                            }
                            Some(idx) => {
                                ez.write_var_ref(idx);
                                ez.loc += 1;
                            }
                        }
                    } else if name == "_" {
                        // Wildcard - anonymous variable
                        ez.write_new_var();
                        ez.loc += 1;
                    } else {
                        // Regular atom
                        write_symbol(name.as_bytes(), pdp, ez)?;
                    }
                    continue;
                }

                // Booleans
                if let Some(b) = v.as_bool() {
                    let s = if b { "true" } else { "false" };
                    write_symbol(s.as_bytes(), pdp, ez)?;
                    continue;
                }

                // Long integers
                if let Some(n) = v.as_long() {
                    let s = n.to_string();
                    write_symbol(s.as_bytes(), pdp, ez)?;
                    continue;
                }

                // Floats
                if let Some(f) = v.as_float() {
                    let s = f.to_string();
                    write_symbol(s.as_bytes(), pdp, ez)?;
                    continue;
                }

                // Strings
                if let Some(s) = v.as_string() {
                    let quoted = format!("\"{}\"", s);
                    write_symbol(quoted.as_bytes(), pdp, ez)?;
                    continue;
                }

                // S-expressions
                if let Some(items) = v.as_sexpr() {
                    if items.len() >= 64 {
                        return Err(format!(
                            "Expression has too many children ({}) - MORK arity limit is 63",
                            items.len()
                        ));
                    }
                    let arity = items.len() as u8;
                    ez.write_arity(arity);
                    ez.loc += 1;

                    if !items.is_empty() {
                        // Push marker to track when we're done with this S-expr (not needed for writing)
                        // Process children in reverse order (stack is LIFO)
                        for item in items.iter().rev() {
                            work_stack.push(WorkItem::Process(item));
                        }
                    }
                    continue;
                }

                // Conjunctions - written as (, goal1 goal2 ...)
                if let Some(goals) = v.as_conjunction() {
                    let total = goals.len() + 1;
                    if total >= 64 {
                        return Err(format!(
                            "Conjunction has too many goals ({}) - MORK arity limit is 63",
                            goals.len()
                        ));
                    }
                    ez.write_arity(total as u8);
                    ez.loc += 1;

                    // Write comma symbol first
                    write_symbol(b",", pdp, ez)?;

                    // Process goals in reverse order
                    for goal in goals.iter().rev() {
                        work_stack.push(WorkItem::Process(goal));
                    }
                    continue;
                }

                // Unit - empty list in MORK
                if v.is_unit() {
                    ez.write_arity(0);
                    ez.loc += 1;
                    continue;
                }

                // Errors - (error "msg" details)
                if let Some((msg, details)) = v.as_error() {
                    ez.write_arity(3);
                    ez.loc += 1;
                    write_symbol(b"error", pdp, ez)?;
                    write_symbol(format!("\"{}\"", msg).as_bytes(), pdp, ez)?;
                    work_stack.push(WorkItem::Process(details));
                    continue;
                }

                // Types - recurse into inner value
                if let Some(inner) = v.as_type() {
                    work_stack.push(WorkItem::Process(inner));
                    continue;
                }

                // Space handles - (Space id name)
                if let Some(handle) = v.as_space() {
                    ez.write_arity(3);
                    ez.loc += 1;
                    write_symbol(b"Space", pdp, ez)?;
                    write_symbol(handle.id.to_string().as_bytes(), pdp, ez)?;
                    write_symbol(format!("\"{}\"", handle.name).as_bytes(), pdp, ez)?;
                    continue;
                }

                // State handles - (State id)
                if let Some(id) = v.as_state() {
                    ez.write_arity(2);
                    ez.loc += 1;
                    write_symbol(b"State", pdp, ez)?;
                    write_symbol(id.to_string().as_bytes(), pdp, ez)?;
                    continue;
                }

                // Memo tables - cannot be stored in MORK
                if let Some(handle) = v.as_memo() {
                    return Err(format!(
                        "Cannot convert Memo table '{}' (id={}) to MORK - memoization tables are runtime-only",
                        handle.name, handle.id
                    ));
                }

                // Empty sentinel - should be filtered before MORK conversion
                if v.is_empty() {
                    return Err(
                        "Cannot convert Empty sentinel to MORK - Empty should be filtered at result collection".to_string()
                    );
                }

                // Fallback - unknown value type
                return Err(format!(
                    "Unsupported value type for MORK conversion: {}",
                    v.friendly_type_name()
                ));
            }
        }
    }

    Ok(())
}

/// Convert MORK bindings to Mettatron Bindings format
///
/// MORK uses BTreeMap<(u8, u8), ExprEnv> where the key is (old_var, new_var).
/// We need to convert this to SmallVec<[(String, MettaValue); 8]> using the original variable names.
///
/// FIXED: Uses mork_expr_to_metta_value() instead of serialize2() to avoid reserved byte panic
/// Now properly reports conversion errors instead of silently skipping bindings.
#[allow(unused_variables)]
pub fn mork_bindings_to_metta<V: Clone + Default + Send + Sync + Unpin>(
    mork_bindings: &std::collections::BTreeMap<(u8, u8), ExprEnv>,
    ctx: &ConversionContext,
    space: &Space<V>,
) -> Result<Bindings, String> {
    trace!(target: "mettatron::conversion::mork_bindings_to_metta", ?mork_bindings);

    use super::environment::MettaEnvironment;

    let mut bindings = Bindings::new();
    let mut conversion_errors: Vec<String> = Vec::new();

    for (&(old_var, _new_var), expr_env) in mork_bindings {
        // Get the variable name from context
        if (old_var as usize) >= ctx.var_names.len() {
            warn!(
                target: "mettatron::conversion::mork_bindings_to_metta",
                old_var, max_vars = ctx.var_names.len(),
                "Variable index exceeds known variables - internal inconsistency detected"
            );

            // Variable index out of bounds - this indicates an internal inconsistency
            conversion_errors.push(format!(
                "Variable index {} exceeds known variables (max: {})",
                old_var,
                ctx.var_names.len().saturating_sub(1)
            ));
            continue;
        }
        let var_name = &ctx.var_names[old_var as usize];

        // Convert MORK Expr directly to MettaValue
        // FIXED: Use mork_expr_to_metta_value() instead of serialize2()
        // This avoids the "reserved byte" panic when bindings contain symbols with reserved bytes
        let expr: Expr = expr_env.subsexpr();
        match MettaEnvironment::mork_expr_to_metta_value(&expr, space) {
            Ok(value) => {
                bindings.insert(format!("${}", var_name), value);
            }
            Err(e) => {
                debug!(
                    target: "mettatron::conversion::mork_bindings_to_metta",
                    var_name = %var_name, error = %e, "Failed to convert individual binding"
                );
                conversion_errors.push(format!(
                    "Failed to convert binding for ${}: {}",
                    var_name, e
                ));
            }
        }
    }

    // If there were any conversion errors, return an error with all failures listed
    if !conversion_errors.is_empty() {
        let errors = conversion_errors.join("\n  - ");
        warn!(
            target: "mettatron::conversion::mork_bindings_to_metta",
            errors, "MORK binding conversion partially failed"
        );
        return Err(format!("MORK binding conversion failed:\n  - {}", errors));
    }

    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;

    #[test]
    fn test_simple_atom_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let mut ctx = ConversionContext::new();

        let atom = MettaValue::Atom("foo".to_string());
        let result = metta_to_mork_bytes(&atom, &space, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_variable_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let mut ctx = ConversionContext::new();

        // First occurrence should create NewVar
        let var = MettaValue::Atom("$x".to_string());
        let result = metta_to_mork_bytes(&var, &space, &mut ctx);
        assert!(result.is_ok());
        assert_eq!(ctx.var_names.len(), 1);
        assert_eq!(ctx.var_names[0], "x");
    }

    #[test]
    fn test_sexpr_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let mut ctx = ConversionContext::new();

        // (double $x)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let result = metta_to_mork_bytes(&sexpr, &space, &mut ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_repeated_variable() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let mut ctx = ConversionContext::new();

        // (* $x $x) - same variable twice
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let result = metta_to_mork_bytes(&sexpr, &space, &mut ctx);
        assert!(result.is_ok());
        // Should only have one variable in context
        assert_eq!(ctx.var_names.len(), 1);
    }

    // =========================================================================
    // Generic MORK Conversion Tests
    // =========================================================================

    #[test]
    fn test_generic_simple_atom_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();
        let mut ctx = ConversionContext::new();

        let atom = MettaValue::Atom("foo".to_string());
        // Test generic version produces same result as original
        let original_result = metta_to_mork_bytes(&atom, &space, &mut ctx);
        assert!(original_result.is_ok());

        let mut generic_ctx = ConversionContext::new();
        let generic_result = value_to_mork_bytes_generic(&atom, &space, &mut generic_ctx);
        assert!(generic_result.is_ok());

        // Both should produce identical bytes
        assert_eq!(original_result.unwrap(), generic_result.unwrap());
    }

    #[test]
    fn test_generic_variable_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();

        let var = MettaValue::Atom("$x".to_string());

        let mut ctx1 = ConversionContext::new();
        let original_result = metta_to_mork_bytes(&var, &space, &mut ctx1);
        assert!(original_result.is_ok());

        let mut ctx2 = ConversionContext::new();
        let generic_result = value_to_mork_bytes_generic(&var, &space, &mut ctx2);
        assert!(generic_result.is_ok());

        assert_eq!(original_result.unwrap(), generic_result.unwrap());
        assert_eq!(ctx1.var_names, ctx2.var_names);
    }

    #[test]
    fn test_generic_sexpr_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();

        // (double $x)
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let mut ctx1 = ConversionContext::new();
        let original_result = metta_to_mork_bytes(&sexpr, &space, &mut ctx1);
        assert!(original_result.is_ok());

        let mut ctx2 = ConversionContext::new();
        let generic_result = value_to_mork_bytes_generic(&sexpr, &space, &mut ctx2);
        assert!(generic_result.is_ok());

        assert_eq!(original_result.unwrap(), generic_result.unwrap());
    }

    #[test]
    fn test_generic_complex_nested_sexpr() {
        let env = MettaEnvironment::default();
        let space = env.create_space();

        // (exec P0 (, (a $x) (b $x)) (, (c $x)))
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![
                MettaValue::SExpr(vec![
                    MettaValue::Atom("a".to_string()),
                    MettaValue::Atom("$x".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("b".to_string()),
                    MettaValue::Atom("$x".to_string()),
                ]),
            ]),
            MettaValue::Conjunction(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("$x".to_string()),
            ])]),
        ]);

        let mut ctx1 = ConversionContext::new();
        let original_result = metta_to_mork_bytes(&sexpr, &space, &mut ctx1);
        assert!(original_result.is_ok());

        let mut ctx2 = ConversionContext::new();
        let generic_result = value_to_mork_bytes_generic(&sexpr, &space, &mut ctx2);
        assert!(generic_result.is_ok());

        // Both should produce identical bytes
        assert_eq!(
            original_result.unwrap(),
            generic_result.unwrap(),
            "Generic conversion must match original for complex nested expressions"
        );
        // Both should track the same variable
        assert_eq!(ctx1.var_names, ctx2.var_names);
    }

    #[test]
    fn test_generic_ground_types() {
        let env = MettaEnvironment::default();
        let space = env.create_space();

        // Test various ground types
        let values = vec![
            MettaValue::Bool(true),
            MettaValue::Bool(false),
            MettaValue::Long(42),
            MettaValue::Long(-123),
            MettaValue::Float(3.14159),
            MettaValue::String("hello world".to_string()),
            MettaValue::Unit(),
            MettaValue::Unit(),
        ];

        for value in values {
            let mut ctx1 = ConversionContext::new();
            let original = metta_to_mork_bytes(&value, &space, &mut ctx1);

            let mut ctx2 = ConversionContext::new();
            let generic = value_to_mork_bytes_generic(&value, &space, &mut ctx2);

            assert!(original.is_ok(), "Original conversion failed for {:?}", value);
            assert!(generic.is_ok(), "Generic conversion failed for {:?}", value);
            assert_eq!(
                original.unwrap(),
                generic.unwrap(),
                "Mismatch for {:?}",
                value
            );
        }
    }

    #[test]
    fn test_generic_error_conversion() {
        let env = MettaEnvironment::default();
        let space = env.create_space();

        // (error "test error" (details here))
        let error = MettaValue::Error(
            "test error".to_string(),
            MettaValue::SExpr(vec![
                MettaValue::Atom("details".to_string()),
                MettaValue::Atom("here".to_string()),
            ]),
        );

        let mut ctx1 = ConversionContext::new();
        let original = metta_to_mork_bytes(&error, &space, &mut ctx1);

        let mut ctx2 = ConversionContext::new();
        let generic = value_to_mork_bytes_generic(&error, &space, &mut ctx2);

        assert!(original.is_ok());
        assert!(generic.is_ok());
        assert_eq!(original.unwrap(), generic.unwrap());
    }
}
