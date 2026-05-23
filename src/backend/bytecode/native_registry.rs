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
//! # Generic Support
//!
//! The registry supports generic value types through `GenericNativeRegistry<V, F>`,
//! enabling zero-conversion execution with MettaValue or MettaValue.
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
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backend::environment::GenericEnvironment;
#[cfg(test)]
use crate::backend::models::MettaValueInner;
use crate::backend::models::{
    global_factory, GcFactory, MettaValue, MettaValueFactory, MettaValueTrait,
};

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

// =============================================================================
// Generic Types (for zero-conversion execution)
// =============================================================================

/// Generic result type for native function calls
pub type GenericNativeResult<V> = Result<Vec<V>, NativeError>;

/// Generic context provided to native functions during execution
#[derive(Clone)]
pub struct GenericNativeContext<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Current environment (for accessing bindings if needed)
    pub env: GenericEnvironment<V, F>,
    /// Factory for constructing values
    pub factory: F,
}

impl<V, F> GenericNativeContext<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Create a new generic native context
    pub fn new(env: GenericEnvironment<V, F>, factory: F) -> Self {
        Self { env, factory }
    }
}

/// Generic type alias for native function signature
pub type GenericNativeFn<V, F> =
    Arc<dyn Fn(&[V], &GenericNativeContext<V, F>) -> GenericNativeResult<V> + Send + Sync>;

/// Generic registry entry for a native function
struct GenericRegistryEntry<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    name: String,
    func: GenericNativeFn<V, F>,
}

/// Generic registry for native Rust functions callable from bytecode
///
/// Functions are registered by name and assigned sequential IDs starting from 0.
/// The registry is append-only; functions cannot be removed or reassigned.
pub struct GenericNativeRegistry<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Functions stored by ID (index)
    functions: Vec<GenericRegistryEntry<V, F>>,
    /// Name to ID mapping for registration lookup
    name_to_id: HashMap<String, u16>,
    /// Phantom data for factory type
    _phantom: PhantomData<F>,
}

impl<V, F> std::fmt::Debug for GenericNativeRegistry<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericNativeRegistry")
            .field("function_count", &self.functions.len())
            .field("names", &self.name_to_id.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl<V, F> Default for GenericNativeRegistry<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<V, F> GenericNativeRegistry<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            name_to_id: HashMap::new(),
            _phantom: PhantomData,
        }
    }

    /// Create a registry with standard library functions pre-registered
    pub fn with_stdlib(factory: F) -> Self {
        let mut registry = Self::new();
        registry.register_stdlib(factory);
        registry
    }

    /// Register a native function, returning its ID
    ///
    /// If a function with this name already exists, returns its existing ID.
    pub fn register<Func>(&mut self, name: &str, func: Func) -> u16
    where
        Func:
            Fn(&[V], &GenericNativeContext<V, F>) -> GenericNativeResult<V> + Send + Sync + 'static,
    {
        // Check if already registered
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }

        let id = self.functions.len() as u16;
        self.functions.push(GenericRegistryEntry {
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
    pub fn call(
        &self,
        id: u16,
        args: &[V],
        ctx: &GenericNativeContext<V, F>,
    ) -> GenericNativeResult<V> {
        let entry = self
            .functions
            .get(id as usize)
            .ok_or(NativeError::NotFound(id))?;

        (entry.func)(args, ctx)
    }

    /// Look up the name of a registered function by ID.
    ///
    /// Returns `None` if the ID is out of range.
    pub fn name_for_id(&self, id: u16) -> Option<&str> {
        self.functions.get(id as usize).map(|e| e.name.as_str())
    }

    /// Get the number of registered functions
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Register standard library functions using generic factory methods
    fn register_stdlib(&mut self, factory: F) {
        // Print function
        self.register("print", |args, _ctx| {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                // Use Display trait via type_name for debug output
                if let Some(s) = arg.as_string() {
                    print!("{}", s);
                } else if let Some(n) = arg.as_long() {
                    print!("{}", n);
                } else if let Some(b) = arg.as_bool() {
                    print!("{}", if b { "true" } else { "false" });
                } else if arg.is_unit() {
                    print!("()");
                } else if let Some(name) = arg.as_atom() {
                    print!("{}", name);
                } else {
                    print!("<{}>", arg.type_name());
                }
            }
            println!();
            Ok(vec![_ctx.factory.unit()])
        });

        // String concatenation
        {
            let factory_clone = factory.clone();
            self.register("concat", move |args, _ctx| {
                let mut result = String::new();
                for arg in args {
                    if let Some(s) = arg.as_string() {
                        result.push_str(s);
                    } else if let Some(n) = arg.as_long() {
                        result.push_str(&n.to_string());
                    } else if let Some(b) = arg.as_bool() {
                        result.push_str(if b { "true" } else { "false" });
                    } else if let Some(name) = arg.as_atom() {
                        result.push_str(name);
                    } else {
                        result.push_str(&format!("<{}>", arg.type_name()));
                    }
                }
                Ok(vec![factory_clone.string(&result)])
            });
        }

        // String length
        {
            let factory_clone = factory.clone();
            self.register("strlen", move |args, _ctx| {
                if args.len() != 1 {
                    return Err(NativeError::ArityMismatch {
                        expected: 1,
                        got: args.len(),
                    });
                }
                if let Some(s) = args[0].as_string() {
                    Ok(vec![factory_clone.long(s.len() as i64)])
                } else {
                    Err(NativeError::TypeError {
                        expected: "String",
                        got: args[0].type_name().to_string(),
                    })
                }
            });
        }

        // Random number
        {
            let factory_clone = factory.clone();
            self.register("random", move |args, _ctx| {
                let max = match args.first() {
                    Some(v) => {
                        if let Some(n) = v.as_long() {
                            n
                        } else {
                            return Err(NativeError::TypeError {
                                expected: "Long",
                                got: v.type_name().to_string(),
                            });
                        }
                    }
                    None => 100, // Default max
                };

                // Simple LCG random (for reproducibility in tests)
                let seed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(42) as u64;

                let random_val =
                    ((seed * 6364136223846793005 + 1442695040888963407) % (max as u64)) as i64;
                Ok(vec![factory_clone.long(random_val)])
            });
        }

        // Assert function
        {
            let factory_clone = factory.clone();
            self.register("assert", move |args, _ctx| {
                if args.len() != 1 && args.len() != 2 {
                    return Err(NativeError::ArityMismatch {
                        expected: 1,
                        got: args.len(),
                    });
                }

                if let Some(b) = args[0].as_bool() {
                    if b {
                        Ok(vec![factory_clone.unit()])
                    } else {
                        let msg = if args.len() > 1 {
                            if let Some(s) = args[1].as_string() {
                                s.to_string()
                            } else {
                                "assertion failed".to_string()
                            }
                        } else {
                            "assertion failed".to_string()
                        };
                        Err(NativeError::RuntimeError(msg))
                    }
                } else {
                    Err(NativeError::TypeError {
                        expected: "Bool",
                        got: args[0].type_name().to_string(),
                    })
                }
            });
        }

        // Type-of function (returns type as atom)
        {
            let factory_clone = factory.clone();
            self.register("type-of", move |args, _ctx| {
                if args.len() != 1 {
                    return Err(NativeError::ArityMismatch {
                        expected: 1,
                        got: args.len(),
                    });
                }

                let type_name = args[0].type_name();
                Ok(vec![factory_clone.atom(type_name)])
            });
        }

        // List operations
        {
            let factory_clone = factory.clone();
            self.register("list-length", move |args, _ctx| {
                if args.len() != 1 {
                    return Err(NativeError::ArityMismatch {
                        expected: 1,
                        got: args.len(),
                    });
                }

                if let Some(items) = args[0].as_sexpr() {
                    Ok(vec![factory_clone.long(items.len() as i64)])
                } else {
                    Err(NativeError::TypeError {
                        expected: "Expression",
                        got: args[0].type_name().to_string(),
                    })
                }
            });
        }

        // Range function: (range start end) -> (start start+1 ... end-1)
        {
            let factory_clone = factory.clone();
            self.register("range", move |args, _ctx| {
                if args.len() != 2 {
                    return Err(NativeError::ArityMismatch {
                        expected: 2,
                        got: args.len(),
                    });
                }

                let start = if let Some(n) = args[0].as_long() {
                    n
                } else {
                    return Err(NativeError::TypeError {
                        expected: "Long",
                        got: args[0].type_name().to_string(),
                    });
                };

                let end = if let Some(n) = args[1].as_long() {
                    n
                } else {
                    return Err(NativeError::TypeError {
                        expected: "Long",
                        got: args[1].type_name().to_string(),
                    });
                };

                let items: Vec<V> = (start..end).map(|n| factory_clone.long(n)).collect();
                Ok(vec![factory_clone.sexpr(items)])
            });
        }
    }
}

// =============================================================================
// Concrete Type Aliases (MettaValue specializations of generic types)
// =============================================================================

/// Result type for native function calls.
pub type NativeResult = GenericNativeResult<MettaValue>;

/// Context provided to native functions during execution.
pub type NativeContext = GenericNativeContext<MettaValue, GcFactory>;

/// Type alias for native function signature.
pub type NativeFn = GenericNativeFn<MettaValue, GcFactory>;

/// Registry for native Rust functions callable from bytecode.
///
/// Functions are registered by name and assigned sequential IDs starting from 0.
/// The registry is append-only; functions cannot be removed or reassigned.
pub type NativeRegistry = GenericNativeRegistry<MettaValue, GcFactory>;

impl Default for NativeContext {
    fn default() -> Self {
        let factory = global_factory();
        Self::new(GenericEnvironment::new(factory), factory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_call() {
        let mut registry = NativeRegistry::new();

        let id = registry.register("add2", |args, _ctx| {
            let a = match args.first() {
                Some(v) => match v.inner() {
                    MettaValueInner::Long(n) => *n,
                    _ => 0,
                },
                _ => 0,
            };
            let b = match args.get(1) {
                Some(v) => match v.inner() {
                    MettaValueInner::Long(n) => *n,
                    _ => 0,
                },
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
        let registry = NativeRegistry::with_stdlib(global_factory());
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
        let registry = NativeRegistry::with_stdlib(global_factory());
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
