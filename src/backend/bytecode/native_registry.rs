//! Native Function Registry
//!
//! This module provides a registry for native Rust functions that can be called
//! from bytecode via the `CallNative` opcode.
//!
//! # Design
//!
//! Native functions are registered by name and assigned a unique 16-bit ID.
//! The VM calls functions by ID for efficient dispatch. Function signatures
//! follow a standard pattern: `fn(&[MettaValue], &NativeContext) -> NativeResult`.
//!
//! # Example
//!
//! ```ignore
//! let mut registry = NativeRegistry::new();
//!
//! // Register a native function
//! let id = registry.register("my_func", |args, ctx| {
//!     let sum = args.iter()
//!         .filter_map(|v| v.as_long())
//!         .sum::<i64>();
//!     Ok(vec![MettaValue::Long(sum)])
//! });
//!
//! // Call by ID during VM execution
//! let result = registry.call(id, &args, &ctx)?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::models::MettaValue;
use crate::backend::Environment;

/// Result type for native function calls
pub type NativeResult = Result<Vec<MettaValue>, NativeError>;

/// Error type for native function calls
#[derive(Debug, Clone)]
pub enum NativeError {
    /// Wrong number of arguments
    ArityMismatch { expected: usize, got: usize },
    /// Type error in arguments
    TypeError { expected: &'static str, got: String },
    /// Runtime error during execution
    RuntimeError(String),
    /// Function not found
    NotFound(u16),
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArityMismatch { expected, got } => {
                write!(f, "arity mismatch: expected {}, got {}", expected, got)
            }
            Self::TypeError { expected, got } => {
                write!(f, "type error: expected {}, got {}", expected, got)
            }
            Self::RuntimeError(msg) => write!(f, "runtime error: {}", msg),
            Self::NotFound(id) => write!(f, "native function {} not found", id),
        }
    }
}

impl std::error::Error for NativeError {}

/// Context provided to native functions during execution
#[derive(Clone)]
pub struct NativeContext {
    /// Current environment (for accessing bindings if needed)
    pub env: Environment,
}

impl NativeContext {
    /// Create a new native context
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

/// Type alias for native function signature
pub type NativeFn = Arc<dyn Fn(&[MettaValue], &NativeContext) -> NativeResult + Send + Sync>;

/// Registry entry for a native function
struct RegistryEntry {
    name: String,
    func: NativeFn,
}

/// Registry for native Rust functions callable from bytecode
///
/// Functions are registered by name and assigned sequential IDs starting from 0.
/// The registry is append-only; functions cannot be removed or reassigned.
pub struct NativeRegistry {
    /// Functions stored by ID (index)
    functions: Vec<RegistryEntry>,
    /// Name to ID mapping for registration lookup
    name_to_id: HashMap<String, u16>,
}

impl std::fmt::Debug for NativeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeRegistry")
            .field("function_count", &self.functions.len())
            .field("names", &self.name_to_id.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for NativeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            name_to_id: HashMap::new(),
        }
    }

    /// Create a registry with standard library functions pre-registered
    pub fn with_stdlib() -> Self {
        let mut registry = Self::new();
        registry.register_stdlib();
        registry
    }

    /// Register a native function, returning its ID
    ///
    /// If a function with this name already exists, returns its existing ID.
    pub fn register<F>(&mut self, name: &str, func: F) -> u16
    where
        F: Fn(&[MettaValue], &NativeContext) -> NativeResult + Send + Sync + 'static,
    {
        // Check if already registered
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }

        let id = self.functions.len() as u16;
        self.functions.push(RegistryEntry {
            name: name.to_string(),
            func: Arc::new(func),
        });
        self.name_to_id.insert(name.to_string(), id);
        id
    }

    /// Get the ID of a registered function by name
    pub fn get_id(&self, name: &str) -> Option<u16> {
        self.name_to_id.get(name).copied()
    }

    /// Get the name of a registered function by ID
    pub fn get_name(&self, id: u16) -> Option<&str> {
        self.functions.get(id as usize).map(|e| e.name.as_str())
    }

    /// Call a native function by ID
    pub fn call(&self, id: u16, args: &[MettaValue], ctx: &NativeContext) -> NativeResult {
        let entry = self
            .functions
            .get(id as usize)
            .ok_or(NativeError::NotFound(id))?;

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

    /// Register standard library functions
    fn register_stdlib(&mut self) {
        // Print function
        self.register("print", |args, _ctx| {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{:?}", arg);
            }
            println!();
            Ok(vec![MettaValue::Unit])
        });

        // String concatenation
        self.register("concat", |args, _ctx| {
            let mut result = String::new();
            for arg in args {
                match arg {
                    MettaValue::String(s) => result.push_str(s),
                    other => result.push_str(&format!("{:?}", other)),
                }
            }
            Ok(vec![MettaValue::String(result)])
        });

        // String length
        self.register("strlen", |args, _ctx| {
            if args.len() != 1 {
                return Err(NativeError::ArityMismatch {
                    expected: 1,
                    got: args.len(),
                });
            }
            match &args[0] {
                MettaValue::String(s) => Ok(vec![MettaValue::Long(s.len() as i64)]),
                other => Err(NativeError::TypeError {
                    expected: "String",
                    got: other.type_name().to_string(),
                }),
            }
        });

        // Random number
        self.register("random", |args, _ctx| {
            let max = match args.first() {
                Some(MettaValue::Long(n)) => *n,
                Some(other) => {
                    return Err(NativeError::TypeError {
                        expected: "Long",
                        got: other.type_name().to_string(),
                    })
                }
                None => 100, // Default max
            };

            // Simple LCG random (for reproducibility in tests)
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(42) as u64;

            let random_val =
                ((seed * 6364136223846793005 + 1442695040888963407) % (max as u64)) as i64;
            Ok(vec![MettaValue::Long(random_val)])
        });

        // Assert function
        self.register("assert", |args, _ctx| {
            if args.len() != 1 && args.len() != 2 {
                return Err(NativeError::ArityMismatch {
                    expected: 1,
                    got: args.len(),
                });
            }

            match &args[0] {
                MettaValue::Bool(true) => Ok(vec![MettaValue::Unit]),
                MettaValue::Bool(false) => {
                    let msg = args
                        .get(1)
                        .map(|v| format!("{:?}", v))
                        .unwrap_or_else(|| "assertion failed".to_string());
                    Err(NativeError::RuntimeError(msg))
                }
                other => Err(NativeError::TypeError {
                    expected: "Bool",
                    got: other.type_name().to_string(),
                }),
            }
        });

        // Type-of function (returns type as atom)
        self.register("type-of", |args, _ctx| {
            if args.len() != 1 {
                return Err(NativeError::ArityMismatch {
                    expected: 1,
                    got: args.len(),
                });
            }

            let type_name = args[0].type_name();
            Ok(vec![MettaValue::Atom(type_name.to_string())])
        });

        // List operations
        self.register("list-length", |args, _ctx| {
            if args.len() != 1 {
                return Err(NativeError::ArityMismatch {
                    expected: 1,
                    got: args.len(),
                });
            }

            match &args[0] {
                MettaValue::SExpr(items) => Ok(vec![MettaValue::Long(items.len() as i64)]),
                other => Err(NativeError::TypeError {
                    expected: "Expression",
                    got: other.type_name().to_string(),
                }),
            }
        });

        // Range function: (range start end) -> (start start+1 ... end-1)
        self.register("range", |args, _ctx| {
            if args.len() != 2 {
                return Err(NativeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                });
            }

            let start = match &args[0] {
                MettaValue::Long(n) => *n,
                other => {
                    return Err(NativeError::TypeError {
                        expected: "Long",
                        got: other.type_name().to_string(),
                    })
                }
            };

            let end = match &args[1] {
                MettaValue::Long(n) => *n,
                other => {
                    return Err(NativeError::TypeError {
                        expected: "Long",
                        got: other.type_name().to_string(),
                    })
                }
            };

            let items: Vec<MettaValue> = (start..end).map(MettaValue::Long).collect();
            Ok(vec![MettaValue::SExpr(items)])
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_call() {
        let mut registry = NativeRegistry::new();

        let id = registry.register("add2", |args, _ctx| {
            let a = match args.get(0) {
                Some(MettaValue::Long(n)) => *n,
                _ => 0,
            };
            let b = match args.get(1) {
                Some(MettaValue::Long(n)) => *n,
                _ => 0,
            };
            Ok(vec![MettaValue::Long(a + b)])
        });

        assert_eq!(id, 0);

        let ctx = NativeContext::default();
        let result = registry
            .call(id, &[MettaValue::Long(10), MettaValue::Long(32)], &ctx)
            .expect("call should succeed");

        assert_eq!(result, vec![MettaValue::Long(42)]);
    }

    #[test]
    fn test_duplicate_registration() {
        let mut registry = NativeRegistry::new();

        let id1 = registry.register("foo", |_args, _ctx| Ok(vec![MettaValue::Long(1)]));
        let id2 = registry.register("foo", |_args, _ctx| Ok(vec![MettaValue::Long(2)]));

        // Should return same ID
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_function_not_found() {
        let registry = NativeRegistry::new();
        let ctx = NativeContext::default();

        let result = registry.call(999, &[], &ctx);
        assert!(matches!(result, Err(NativeError::NotFound(999))));
    }

    #[test]
    fn test_stdlib() {
        let registry = NativeRegistry::with_stdlib();
        let ctx = NativeContext::default();

        // Test concat
        let concat_id = registry
            .get_id("concat")
            .expect("concat should be registered");
        let result = registry
            .call(
                concat_id,
                &[
                    MettaValue::String("hello".to_string()),
                    MettaValue::String(" world".to_string()),
                ],
                &ctx,
            )
            .expect("concat should succeed");

        assert_eq!(result, vec![MettaValue::String("hello world".to_string())]);
    }

    #[test]
    fn test_range() {
        let registry = NativeRegistry::with_stdlib();
        let ctx = NativeContext::default();

        let range_id = registry
            .get_id("range")
            .expect("range should be registered");
        let result = registry
            .call(range_id, &[MettaValue::Long(0), MettaValue::Long(5)], &ctx)
            .expect("range should succeed");

        let expected = vec![MettaValue::SExpr(vec![
            MettaValue::Long(0),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
            MettaValue::Long(4),
        ])];

        assert_eq!(result, expected);
    }
}
