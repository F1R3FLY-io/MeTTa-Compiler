//! JSON encoding/decoding grounded operations.
//!
//! HE-bisim source: `hyperon-experimental/lib/src/metta/runner/builtin_mods/json.rs`.
//!
//! Two operations registered:
//!   - `(json-encode <atom>) -> String`
//!   - `(json-decode <String>) -> <atom>`
//!
//! HE empirical:
//!   ```text
//!   !(import! &self json)
//!   !(json-encode 42)                         => ["42"]
//!   !(json-encode "abc")                      => ["\"abc\""]
//!   !(json-encode (1 2 3))                    => ["[1, 2, 3]"]
//!   !(json-decode "42")                       => [42]
//!   !(json-decode "[1, 2, 3]")                => [(1 2 3)]
//!   !(json-decode (json-encode 42))           => [42]
//!   ```
//!
//! Encoding rules (HE empirical):
//!   - `Long(n)`           -> JSON integer
//!   - `Float(f)`          -> JSON float
//!   - `Bool(b)`           -> JSON boolean
//!   - `String(s)`         -> JSON string (serialized verbatim; quotes added by serde_json)
//!   - `Atom("null")`      -> JSON null
//!   - `Atom(sym)`         -> JSON string `"sym!:<sym>"`
//!   - `Variable("$v")`    -> JSON string `"var!:<v>"` (treated like a symbol)
//!   - `SExpr([a, b, ...])` -> JSON array `[a', b', ...]` (recursive)
//!
//! Decoding inverts this. The dict-space encoding from HE (iterating a
//! GroundingSpace's `(key value)` atoms) is not implemented in this initial
//! port — the conformance fixture 019 only round-trips a number atom, and
//! the spec test catalog does not yet require space encoding/decoding.

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use crate::backend::grounded::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// `(json-encode <atom>) -> String`.
pub struct JsonEncodeOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for JsonEncodeOp {
    fn name(&self) -> &str {
        "json-encode"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let arg = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    let json = match encode_value(value) {
                        Ok(json) => json,
                        Err(e) => return GroundedWork::Error(e),
                    };
                    results.push((factory.string(&json), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for json-encode operation", state.step),
        }
    }
}

/// `(json-decode <String>) -> <atom>`.
pub struct JsonDecodeOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for JsonDecodeOp {
    fn name(&self) -> &str {
        "json-decode"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let arg = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    let Some(s) = value.as_string() else {
                        // HE empirical: `!(json-decode 5)` =>
                        // `(Error (json-decode 5) (BadArgType 1 String Number))`.
                        return GroundedWork::Error(ExecError::BadArgType {
                            pos: 1,
                            expected: "String",
                            got: value.friendly_type_name().to_string(),
                        });
                    };
                    let decoded: serde_json::Value = match serde_json::from_str(s) {
                        Ok(v) => v,
                        Err(e) => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Failed to decode string. Reason: {}",
                                e
                            )));
                        }
                    };
                    let atom = decode_value(&decoded, factory);
                    results.push((atom, None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for json-decode operation", state.step),
        }
    }
}

/// Encode a single MeTTa value to a JSON string.
///
/// Matches HE's `encode_atom` in `lib/src/metta/runner/builtin_mods/json.rs:145`
/// (excluding the `DynSpace`-as-dict-space path, which is not currently
/// modelled in MeTTaTron's value representation for JSON purposes).
fn encode_value<V: MettaValueTrait>(value: &V) -> Result<String, ExecError> {
    // Scalars are encoded via serde_json directly so escaping/precision
    // match the HE format exactly.
    if let Some(n) = value.as_long() {
        return serde_json::to_string(&n).map_err(|err| {
            ExecError::Runtime(format!("Encode integer failed: {}", err))
        });
    }
    if let Some(f) = value.as_float() {
        return serde_json::to_string(&f).map_err(|err| {
            ExecError::Runtime(format!("Encode float failed: {}", err))
        });
    }
    if let Some(b) = value.as_bool() {
        return serde_json::to_string(&b).map_err(|err| {
            ExecError::Runtime(format!("Encode bool failed: {}", err))
        });
    }
    if let Some(s) = value.as_string() {
        return serde_json::to_string(s).map_err(|err| {
            ExecError::Runtime(format!("Encode string failed: {}", err))
        });
    }
    if let Some(items) = value.as_sexpr() {
        // SExpr -> JSON array. Preallocate capacity for HE-bisim
        // "[a, b, c]" formatting (note the space after each comma).
        let mut parts: Vec<String> = Vec::with_capacity(items.len());
        for item in items {
            parts.push(encode_value(item)?);
        }
        return Ok(format!("[{}]", parts.join(", ")));
    }
    if let Some(name) = value.as_atom() {
        // HE distinguishes "null" symbol from other symbols.
        if name == "null" {
            return Ok("null".to_string());
        }
        // Variables start with $ in MeTTa. HE prefixes "var!:" for
        // those and "sym!:" for plain symbols.
        let prefixed = if name.starts_with('$') {
            // Strip the leading '$' to match HE's `var.name()`.
            format!("var!:{}", &name[1..])
        } else {
            format!("sym!:{}", name)
        };
        return serde_json::to_string(&prefixed).map_err(|err| {
            ExecError::Runtime(format!("Encode symbol failed: {}", err))
        });
    }

    Err(ExecError::Runtime(format!(
        "Encode failed for atom (unsupported type): {}",
        value.friendly_type_name()
    )))
}

/// Decode a serde_json `Value` into a MeTTa value via `factory`.
fn decode_value<V, F>(v: &serde_json::Value, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    match v {
        serde_json::Value::Null => factory.atom("null"),
        serde_json::Value::Bool(b) => factory.bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                factory.long(i)
            } else if let Some(f) = n.as_f64() {
                factory.float(f)
            } else {
                // u64 (HE returns Err here); we represent as float as a
                // best-effort fallback.
                factory.float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => {
            // HE prefixes "sym!:" for symbols and "var!:" for variables when
            // encoding; decode strips those prefixes and reconstructs the
            // appropriate atom shape.
            if let Some(rest) = s.strip_prefix("sym!:") {
                factory.atom(rest)
            } else if let Some(rest) = s.strip_prefix("var!:") {
                factory.atom(&format!("${}", rest))
            } else {
                factory.string(s)
            }
        }
        serde_json::Value::Array(items) => {
            let mut children: Vec<V> = Vec::with_capacity(items.len());
            for item in items {
                children.push(decode_value(item, factory));
            }
            factory.sexpr(children)
        }
        serde_json::Value::Object(map) => {
            // HE decodes objects into a fresh GroundingSpace of
            // `((key) (value))` pairs. MeTTaTron does not have a generic
            // factory.space() that exposes the same in-rule iteration as HE,
            // so we encode as an SExpr of (key value) pairs. The conformance
            // fixture 019 does not exercise this path.
            let mut pairs: Vec<V> = Vec::with_capacity(map.len());
            for (k, val) in map {
                let key = factory.string(k);
                let decoded = decode_value(val, factory);
                pairs.push(factory.sexpr(vec![key, decoded]));
            }
            factory.sexpr(pairs)
        }
    }
}
