//! External Function Registry
//!
//! This module provides a registry for external functions that can be called
//! from bytecode via the `CallExternal` opcode. External functions are typically
//! dynamically registered at runtime (e.g., from Rholang or other FFI sources).
//!
//! # Design
//!
//! External functions are registered by name and called by name lookup during execution.
//! Unlike native functions (which use numeric IDs for efficiency), external functions
//! use string-based lookup to support dynamic registration from external systems.
//!
//! # Example
//!
//! ```ignore
//! let mut registry = ExternalRegistry::new();
//!
//! // Register an external function
//! registry.register("rholang_send", |args, ctx| {
//!     // Send to Rholang channel
//!     Ok(vec![MettaValue::Unit])
//! });
//!
//! // Call by name during VM execution
//! let result = registry.call("rholang_send", &args, &ctx)?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::models::MettaValue;
use crate::backend::Environment;

/// Result type for external function calls
pub type ExternalResult = Result<Vec<MettaValue>, ExternalError>;

/// Error type for external function calls
#[derive(Debug, Clone)]
pub enum ExternalError {
    /// Wrong number of arguments
    ArityMismatch { expected: usize, got: usize },
    /// Type error in arguments
    TypeError { expected: &'static str, got: String },
    /// Runtime error during execution
    RuntimeError(String),
    /// Function not found
    NotFound(String),
    /// FFI system not available
    NotAvailable(String),
}

impl std::fmt::Display for ExternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArityMismatch { expected, got } => {
                write!(f, "arity mismatch: expected {}, got {}", expected, got)
            }
            Self::TypeError { expected, got } => {
                write!(f, "type error: expected {}, got {}", expected, got)
            }
            Self::RuntimeError(msg) => write!(f, "runtime error: {}", msg),
            Self::NotFound(name) => write!(f, "external function '{}' not found", name),
            Self::NotAvailable(msg) => write!(f, "external function not available: {}", msg),
        }
    }
}

impl std::error::Error for ExternalError {}

/// Context provided to external functions during execution
#[derive(Clone)]
pub struct ExternalContext {
    /// Current environment (for accessing bindings if needed)
    pub env: Environment,
}

impl ExternalContext {
    /// Create a new external context
    pub fn new(env: Environment) -> Self {
        Self { env }
    }

    /// Create a default context with empty environment
    pub fn default() -> Self {
        Self {
            env: Environment::new(),
        }
    }
}

/// Type alias for external function signature
pub type ExternalFn = Arc<dyn Fn(&[MettaValue], &ExternalContext) -> ExternalResult + Send + Sync>;

/// Registry entry for an external function
struct RegistryEntry {
    func: ExternalFn,
}

/// Registry for external functions callable from bytecode
///
/// External functions are registered by name and looked up by name during execution.
/// This supports dynamic registration from external systems like Rholang.
pub struct ExternalRegistry {
    /// Functions stored by name
    functions: HashMap<String, RegistryEntry>,
}

impl std::fmt::Debug for ExternalRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalRegistry")
            .field("function_count", &self.functions.len())
            .field("names", &self.functions.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for ExternalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
        }
    }

    /// Register an external function
    ///
    /// If a function with this name already exists, it will be replaced.
    pub fn register<F>(&mut self, name: &str, func: F)
    where
        F: Fn(&[MettaValue], &ExternalContext) -> ExternalResult + Send + Sync + 'static,
    {
        self.functions.insert(
            name.to_string(),
            RegistryEntry {
                func: Arc::new(func),
            },
        );
    }

    /// Unregister an external function
    ///
    /// Returns true if the function was present and removed.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.functions.remove(name).is_some()
    }

    /// Check if a function is registered
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

    /// Call an external function by name
    pub fn call(&self, name: &str, args: &[MettaValue], ctx: &ExternalContext) -> ExternalResult {
        let entry = self
            .functions
            .get(name)
            .ok_or_else(|| ExternalError::NotFound(name.to_string()))?;

        (entry.func)(args, ctx)
    }

    /// Get the number of registered functions
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Get an iterator over registered function names
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.functions.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_call() {
        let mut registry = ExternalRegistry::new();

        registry.register("double", |args, _ctx| {
            let n = match args.get(0) {
                Some(MettaValue::Long(n)) => *n,
                _ => {
                    return Err(ExternalError::TypeError {
                        expected: "Long",
                        got: "other".to_string(),
                    })
                }
            };
            Ok(vec![MettaValue::Long(n * 2)])
        });

        let ctx = ExternalContext::default();
        let result = registry
            .call("double", &[MettaValue::Long(21)], &ctx)
            .expect("call should succeed");

        assert_eq!(result, vec![MettaValue::Long(42)]);
    }

    #[test]
    fn test_function_not_found() {
        let registry = ExternalRegistry::new();
        let ctx = ExternalContext::default();

        let result = registry.call("nonexistent", &[], &ctx);
        assert!(matches!(result, Err(ExternalError::NotFound(_))));
    }

    #[test]
    fn test_register_replaces() {
        let mut registry = ExternalRegistry::new();

        registry.register("foo", |_args, _ctx| Ok(vec![MettaValue::Long(1)]));
        registry.register("foo", |_args, _ctx| Ok(vec![MettaValue::Long(2)]));

        let ctx = ExternalContext::default();
        let result = registry.call("foo", &[], &ctx).expect("should succeed");

        // Should use the second registration
        assert_eq!(result, vec![MettaValue::Long(2)]);
    }

    #[test]
    fn test_unregister() {
        let mut registry = ExternalRegistry::new();

        registry.register("test", |_args, _ctx| Ok(vec![MettaValue::Unit]));
        assert!(registry.contains("test"));

        let removed = registry.unregister("test");
        assert!(removed);
        assert!(!registry.contains("test"));

        let removed_again = registry.unregister("test");
        assert!(!removed_again);
    }

    #[test]
    fn test_names_iterator() {
        let mut registry = ExternalRegistry::new();

        registry.register("alpha", |_args, _ctx| Ok(vec![]));
        registry.register("beta", |_args, _ctx| Ok(vec![]));
        registry.register("gamma", |_args, _ctx| Ok(vec![]));

        let names: Vec<_> = registry.names().collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }
}
