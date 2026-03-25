//! Pre-compiled bytecode chunks for built-in operations.
//!
//! Provides a lazy-initialized registry mapping `(head, arity)` to
//! pre-compiled `Arc<BytecodeChunk>`. This eliminates compilation overhead
//! for common operations like `(+ a b)`, `(if c t e)`, `(car-atom x)`, etc.
//!
//! Each chunk expects its arguments already on the value stack and ends
//! with a `Return` opcode.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use super::chunk::{BytecodeChunk, GenericChunkBuilder};
use super::opcodes::Opcode;
use crate::backend::models::{GcFactory, MettaValue};

/// Key for looking up pre-compiled built-in chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BuiltinKey {
    head: &'static str,
    arity: u8,
}

static BUILTIN_REGISTRY: OnceLock<HashMap<BuiltinKey, Arc<BytecodeChunk>>> = OnceLock::new();

/// Get a pre-compiled bytecode chunk for a built-in operation.
///
/// Returns `None` if the operation is not a recognized built-in or has wrong arity.
/// The returned chunk expects `arity` arguments already on the value stack.
pub fn get_builtin_chunk(head: &str, arity: u8) -> Option<Arc<BytecodeChunk>> {
    let registry = BUILTIN_REGISTRY.get_or_init(build_registry);
    // Map runtime string to static string for HashMap lookup
    let static_head = intern_head(head)?;
    registry.get(&BuiltinKey { head: static_head, arity }).cloned()
}

/// Map runtime strings to static strings for HashMap lookup.
fn intern_head(head: &str) -> Option<&'static str> {
    match head {
        // Arithmetic
        "+" => Some("+"),
        "-" => Some("-"),
        "*" => Some("*"),
        "/" => Some("/"),
        "%" => Some("%"),
        // Comparison
        "<" => Some("<"),
        ">" => Some(">"),
        "<=" => Some("<="),
        ">=" => Some(">="),
        "==" => Some("=="),
        // Boolean
        "and" => Some("and"),
        "or" => Some("or"),
        "not" => Some("not"),
        "xor" => Some("xor"),
        // List
        "car-atom" => Some("car-atom"),
        "cdr-atom" => Some("cdr-atom"),
        "cons-atom" => Some("cons-atom"),
        "size-atom" => Some("size-atom"),
        "decons-atom" => Some("decons-atom"),
        // Type
        "get-type" => Some("get-type"),
        "get-metatype" => Some("get-metatype"),
        // String
        "repr" => Some("repr"),
        _ => None,
    }
}

fn build_registry() -> HashMap<BuiltinKey, Arc<BytecodeChunk>> {
    let mut map = HashMap::with_capacity(32);
    register_arithmetic(&mut map);
    register_comparison(&mut map);
    register_boolean(&mut map);
    register_list_ops(&mut map);
    register_type_ops(&mut map);
    register_string_ops(&mut map);
    map
}

fn build_binary_op(name: &str, op: Opcode) -> Arc<BytecodeChunk> {
    let mut builder = GenericChunkBuilder::<MettaValue, GcFactory>::new(name);
    builder.set_arity(2);
    builder.emit(op);
    builder.emit(Opcode::Return);
    builder.build_arc()
}

fn build_unary_op(name: &str, op: Opcode) -> Arc<BytecodeChunk> {
    let mut builder = GenericChunkBuilder::<MettaValue, GcFactory>::new(name);
    builder.set_arity(1);
    builder.emit(op);
    builder.emit(Opcode::Return);
    builder.build_arc()
}

fn register_arithmetic(map: &mut HashMap<BuiltinKey, Arc<BytecodeChunk>>) {
    map.insert(BuiltinKey { head: "+", arity: 2 }, build_binary_op("builtin_add", Opcode::Add));
    map.insert(BuiltinKey { head: "-", arity: 2 }, build_binary_op("builtin_sub", Opcode::Sub));
    map.insert(BuiltinKey { head: "*", arity: 2 }, build_binary_op("builtin_mul", Opcode::Mul));
    map.insert(BuiltinKey { head: "/", arity: 2 }, build_binary_op("builtin_div", Opcode::Div));
    map.insert(BuiltinKey { head: "%", arity: 2 }, build_binary_op("builtin_mod", Opcode::Mod));
}

fn register_comparison(map: &mut HashMap<BuiltinKey, Arc<BytecodeChunk>>) {
    map.insert(BuiltinKey { head: "<", arity: 2 }, build_binary_op("builtin_lt", Opcode::Lt));
    map.insert(BuiltinKey { head: ">", arity: 2 }, build_binary_op("builtin_gt", Opcode::Gt));
    map.insert(BuiltinKey { head: "<=", arity: 2 }, build_binary_op("builtin_le", Opcode::Le));
    map.insert(BuiltinKey { head: ">=", arity: 2 }, build_binary_op("builtin_ge", Opcode::Ge));
    map.insert(BuiltinKey { head: "==", arity: 2 }, build_binary_op("builtin_eq", Opcode::Eq));
}

fn register_boolean(map: &mut HashMap<BuiltinKey, Arc<BytecodeChunk>>) {
    map.insert(BuiltinKey { head: "and", arity: 2 }, build_binary_op("builtin_and", Opcode::And));
    map.insert(BuiltinKey { head: "or", arity: 2 }, build_binary_op("builtin_or", Opcode::Or));
    map.insert(BuiltinKey { head: "not", arity: 1 }, build_unary_op("builtin_not", Opcode::Not));
    map.insert(BuiltinKey { head: "xor", arity: 2 }, build_binary_op("builtin_xor", Opcode::Xor));
}

fn register_list_ops(map: &mut HashMap<BuiltinKey, Arc<BytecodeChunk>>) {
    map.insert(BuiltinKey { head: "car-atom", arity: 1 }, build_unary_op("builtin_car", Opcode::GetHead));
    map.insert(BuiltinKey { head: "cdr-atom", arity: 1 }, build_unary_op("builtin_cdr", Opcode::GetTail));
    map.insert(BuiltinKey { head: "cons-atom", arity: 2 }, build_binary_op("builtin_cons", Opcode::ConsAtom));
    map.insert(BuiltinKey { head: "size-atom", arity: 1 }, build_unary_op("builtin_size", Opcode::GetArity));
    map.insert(BuiltinKey { head: "decons-atom", arity: 1 }, build_unary_op("builtin_decons", Opcode::DeconsAtom));
}

fn register_type_ops(map: &mut HashMap<BuiltinKey, Arc<BytecodeChunk>>) {
    map.insert(BuiltinKey { head: "get-type", arity: 1 }, build_unary_op("builtin_get_type", Opcode::GetType));
    map.insert(BuiltinKey { head: "get-metatype", arity: 1 }, build_unary_op("builtin_get_metatype", Opcode::GetMetaType));
}

fn register_string_ops(map: &mut HashMap<BuiltinKey, Arc<BytecodeChunk>>) {
    map.insert(BuiltinKey { head: "repr", arity: 1 }, build_unary_op("builtin_repr", Opcode::Repr));
}
