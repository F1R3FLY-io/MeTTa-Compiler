//! Conversion from live `MettaValue` to owned `TraceValue`.
//!
//! `trace_value()` uses an iterative trampoline to convert arbitrarily
//! deep `MettaValue` trees without risking stack overflow.  Thread-local
//! work/continuation stacks are reused across calls — zero allocation
//! after warmup.

use std::cell::RefCell;
use std::collections::HashMap;

use trace_format::{TraceSpan, TraceValue};

use crate::backend::models::metta_value::{MettaValue, MettaValueInner};
use crate::ir::Span;

// ---------------------------------------------------------------------------
// Continuation for compound-type assembly
// ---------------------------------------------------------------------------

/// What to do after children are converted.
enum Cont {
    /// Collect N children into `SExpr(Vec<TraceValue>)`.
    CollectSExpr {
        remaining: usize,
        collected: Vec<TraceValue>,
    },
    /// Wrap single child in `Error(msg, Box<child>)`.
    WrapError { message: String },
    /// Wrap single child in `Type(Box<child>)`.
    WrapType,
    /// Wrap single child in `Quoted(Box<child>)`.
    WrapQuoted,
    /// Collect N children, then prepend "," atom for Conjunction.
    CollectConjunction {
        remaining: usize,
        collected: Vec<TraceValue>,
    },
}

// ---------------------------------------------------------------------------
// Thread-local reusable stacks
// ---------------------------------------------------------------------------

thread_local! {
    /// Reusable work stack for `trace_value` conversion.
    static TRACE_WORK: RefCell<Vec<*const MettaValueInner>> =
        RefCell::new(Vec::with_capacity(64));

    /// Reusable continuation stack for `trace_value` conversion.
    static TRACE_CONTS: RefCell<Vec<Cont>> =
        RefCell::new(Vec::with_capacity(32));
}

// ---------------------------------------------------------------------------
// Core conversion: MettaValue → TraceValue (iterative trampoline)
// ---------------------------------------------------------------------------

/// Convert a live `MettaValue` into a fully-owned `TraceValue`.
///
/// This performs a deep copy, converting slab-allocated `&'static str`
/// and `&'static [MettaValue]` into owned `String` / `Vec<TraceValue>`.
///
/// # Safety contract
///
/// The raw `*const MettaValueInner` pointers stored on the work stack
/// are safe because every `MettaValue.inner_ref()` points to `'static` slab
/// memory.  The pointers are only dereferenced within the scope of this
/// function call.
pub fn trace_value(root: &MettaValue) -> TraceValue {
    TRACE_WORK.with(|work_cell| {
        TRACE_CONTS.with(|conts_cell| {
            let mut work = work_cell.borrow_mut();
            let mut conts = conts_cell.borrow_mut();

            work.clear();
            conts.clear();

            // Seed with root's inner (auto-strip outermost Spanned via inner()).
            work.push(root.inner() as *const MettaValueInner);

            let mut result: Option<TraceValue> = None;

            loop {
                // Phase 1: Pop work item, convert leaf or push children + continuation.
                if result.is_none() {
                    if let Some(ptr) = work.pop() {
                        // SAFETY: ptr came from MettaValue.inner() which is 'static.
                        let inner = unsafe { &*ptr };
                        // Strip Spanned layers (defensive — inner() should already strip).
                        let inner = strip_spanned(inner);
                        match inner {
                            // Leaf types — immediate TraceValue.
                            MettaValueInner::Atom(s) => {
                                result = Some(TraceValue::Atom(s.to_string()));
                            }
                            MettaValueInner::Bool(b) => {
                                result = Some(TraceValue::Bool(*b));
                            }
                            MettaValueInner::Long(n) => {
                                result = Some(TraceValue::Long(*n));
                            }
                            MettaValueInner::Float(f) => {
                                result = Some(TraceValue::Float(*f));
                            }
                            MettaValueInner::String(s) => {
                                result = Some(TraceValue::String(s.to_string()));
                            }
                            MettaValueInner::Unit => {
                                result = Some(TraceValue::Unit);
                            }
                            MettaValueInner::Empty => {
                                result = Some(TraceValue::Empty);
                            }
                            MettaValueInner::NotReducible => {
                                // %Irreducible% / NotReducible marker — represent
                                // as the canonical atom name for trace inspection.
                                result = Some(TraceValue::Atom("%NotReducible%".into()));
                            }
                            MettaValueInner::Space(_) => {
                                result = Some(TraceValue::Atom("<space>".into()));
                            }
                            MettaValueInner::State(id) => {
                                result = Some(TraceValue::Atom(format!("<state:{id}>")));
                            }
                            MettaValueInner::Memo(_) => {
                                result = Some(TraceValue::Atom("<memo>".into()));
                            }

                            // Compound types — push continuation + children.
                            MettaValueInner::SExpr(items) => {
                                if items.is_empty() {
                                    result = Some(TraceValue::SExpr(Vec::new()));
                                } else {
                                    conts.push(Cont::CollectSExpr {
                                        remaining: items.len(),
                                        collected: Vec::with_capacity(items.len()),
                                    });
                                    // Push in reverse so first child is popped first.
                                    for item in items.iter().rev() {
                                        work.push(item.inner() as *const MettaValueInner);
                                    }
                                    continue;
                                }
                            }
                            MettaValueInner::Error(offending, detail) => {
                                // HE-bisimilar `Error(offending, detail)`. The
                                // serialized trace format keeps the old
                                // `Error(String, Box<TraceValue>)` shape, so
                                // we extract the human-readable message from
                                // the detail slot (falling back to its Display)
                                // and recurse into `offending` as the child.
                                let message = detail
                                    .as_string()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| format!("{}", detail));
                                conts.push(Cont::WrapError { message });
                                work.push(offending.inner() as *const MettaValueInner);
                                continue;
                            }
                            MettaValueInner::Type(inner_v) => {
                                conts.push(Cont::WrapType);
                                work.push(inner_v.inner() as *const MettaValueInner);
                                continue;
                            }
                            MettaValueInner::Quoted(inner_v) => {
                                conts.push(Cont::WrapQuoted);
                                work.push(inner_v.inner() as *const MettaValueInner);
                                continue;
                            }
                            MettaValueInner::Conjunction(items) => {
                                if items.is_empty() {
                                    result =
                                        Some(TraceValue::SExpr(vec![TraceValue::Atom(",".into())]));
                                } else {
                                    conts.push(Cont::CollectConjunction {
                                        remaining: items.len(),
                                        collected: Vec::with_capacity(items.len() + 1),
                                    });
                                    for item in items.iter().rev() {
                                        work.push(item.inner() as *const MettaValueInner);
                                    }
                                    continue;
                                }
                            }
                            MettaValueInner::Spanned(v, _) => {
                                // Should not reach here after strip_spanned, but defensive.
                                work.push(v.inner() as *const MettaValueInner);
                                continue;
                            }
                        }
                    } else if conts.is_empty() {
                        // No work, no conts, no result → shouldn't happen with correct input.
                        return TraceValue::Unit;
                    }
                }

                // Phase 2: Feed result into pending continuation.
                if let Some(child) = result.take() {
                    if conts.is_empty() {
                        // No continuations — child is the final result.
                        return child;
                    }

                    let cont = conts.last_mut().expect("conts non-empty checked above");
                    match cont {
                        Cont::CollectSExpr {
                            remaining,
                            collected,
                        } => {
                            collected.push(child);
                            *remaining -= 1;
                            if *remaining == 0 {
                                let collected = std::mem::take(collected);
                                conts.pop();
                                result = Some(TraceValue::SExpr(collected));
                            } else {
                                continue; // More children to process.
                            }
                        }
                        Cont::WrapError { message } => {
                            let msg = std::mem::take(message);
                            conts.pop();
                            result = Some(TraceValue::Error(msg, Box::new(child)));
                        }
                        Cont::WrapType => {
                            conts.pop();
                            result = Some(TraceValue::Type(Box::new(child)));
                        }
                        Cont::WrapQuoted => {
                            conts.pop();
                            result = Some(TraceValue::Quoted(Box::new(child)));
                        }
                        Cont::CollectConjunction {
                            remaining,
                            collected,
                        } => {
                            collected.push(child);
                            *remaining -= 1;
                            if *remaining == 0 {
                                let mut items = std::mem::take(collected);
                                conts.pop();
                                // Prepend "," atom for conjunction readability.
                                items.insert(0, TraceValue::Atom(",".into()));
                                result = Some(TraceValue::SExpr(items));
                            } else {
                                continue;
                            }
                        }
                    }

                    // Continuation produced a result — loop to feed it upstream.
                    continue;
                }
            }
        })
    })
}

/// Convert any `MettaValueTrait` implementor to an owned `TraceValue`.
///
/// This is a recursive implementation using the trait methods rather than
/// raw `MettaValueInner` access. It is used in generic trampoline code
/// where the value type parameter `C::Value` is `MettaValueTrait` but
/// not necessarily `MettaValue` at the type level. In practice, `C::Value`
/// is always `MettaValue`, but Rust's type system requires the generic path.
///
/// For the hot path with concrete `MettaValue`, prefer `trace_value()` which
/// uses the optimized iterative trampoline with thread-local stacks.
pub fn trace_value_generic<V: crate::backend::models::MettaValueTrait + 'static>(
    v: &V,
) -> TraceValue {
    // Fast path: when V is MettaValue (always true in practice — all EvalContext
    // impls use Value = MettaValue), delegate to the iterative trace_value()
    // which uses thread-local work/continuation stacks and cannot overflow.
    if std::any::TypeId::of::<V>() == std::any::TypeId::of::<crate::backend::models::MettaValue>() {
        // SAFETY: TypeId equality guarantees V == MettaValue.
        let mv: &crate::backend::models::MettaValue =
            unsafe { &*(v as *const V as *const crate::backend::models::MettaValue) };
        return trace_value(mv);
    }

    // Fallback: recursive implementation for non-MettaValue types (unreachable in practice)
    if let Some(s) = v.as_atom() {
        TraceValue::Atom(s.to_string())
    } else if let Some(b) = v.as_bool() {
        TraceValue::Bool(b)
    } else if let Some(n) = v.as_long() {
        TraceValue::Long(n)
    } else if let Some(f) = v.as_float() {
        TraceValue::Float(f)
    } else if let Some(s) = v.as_string() {
        TraceValue::String(s.to_string())
    } else if v.is_unit() {
        TraceValue::Unit
    } else if v.is_empty() {
        TraceValue::Empty
    } else if let Some((offending, detail)) = v.as_error() {
        // HE-bisimilar Error(offending, detail). TraceValue::Error keeps the
        // old `(String, Box<TraceValue>)` shape — we extract the message from
        // the detail slot and recurse into offending as the child.
        // Use Debug formatting for non-String details: V's only universal
        // bound is `MettaValueTrait + Debug`, not Display, so `format!("{}", detail)`
        // doesn't compile for generic V. Debug is sufficient for trace output.
        let message = detail
            .as_string()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{:?}", detail));
        TraceValue::Error(message, Box::new(trace_value_generic(offending)))
    } else if let Some(items) = v.as_sexpr() {
        TraceValue::SExpr(items.iter().map(trace_value_generic).collect())
    } else if let Some(inner) = v.as_type() {
        TraceValue::Type(Box::new(trace_value_generic(inner)))
    } else if let Some(inner) = v.as_quoted_ref() {
        TraceValue::Quoted(Box::new(trace_value_generic(inner)))
    } else if let Some(items) = v.as_conjunction() {
        let mut parts: Vec<TraceValue> = items.iter().map(trace_value_generic).collect();
        parts.insert(0, TraceValue::Atom(",".into()));
        TraceValue::SExpr(parts)
    } else if v.as_space().is_some() {
        TraceValue::Atom("<space>".into())
    } else if let Some(id) = v.as_state() {
        TraceValue::Atom(format!("<state:{id}>"))
    } else if v.as_memo().is_some() {
        TraceValue::Atom("<memo>".into())
    } else {
        TraceValue::Atom(format!("{:?}", v))
    }
}

/// Strip `Spanned` wrappers from a `MettaValueInner` reference.
fn strip_spanned(mut inner: &MettaValueInner) -> &MettaValueInner {
    loop {
        match inner {
            MettaValueInner::Spanned(v, _) => {
                inner = v.inner();
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------
// Span conversion
// ---------------------------------------------------------------------------

/// File table for interning file paths to `u16` IDs.
pub struct FileTable {
    paths: Vec<String>,
    index: HashMap<String, u16>,
}

impl FileTable {
    pub fn new() -> Self {
        Self {
            paths: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Intern a file path and return its `u16` ID.
    pub fn intern(&mut self, path: &str) -> u16 {
        if let Some(&id) = self.index.get(path) {
            return id;
        }
        let id = self.paths.len() as u16;
        self.paths.push(path.to_string());
        self.index.insert(path.to_string(), id);
        id
    }

    /// Get the list of interned paths (for `TraceHeader::file_table`).
    pub fn into_paths(self) -> Vec<String> {
        self.paths
    }

    /// Get a reference to the current paths.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

impl Default for FileTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert an `ir::Span` to a compact `TraceSpan`.
///
/// The file path must already be interned in the `FileTable`.
pub fn trace_span(span: &Span, file_id: u16) -> TraceSpan {
    TraceSpan {
        file_id,
        start_row: span.start.row as u32,
        start_col: span.start.column as u32,
        end_row: span.end.row as u32,
        end_col: span.end.column as u32,
    }
}

// ---------------------------------------------------------------------------
// Bindings conversion
// ---------------------------------------------------------------------------

/// Snapshot a set of variable bindings into owned `(String, TraceValue)` pairs.
pub fn trace_bindings<V, I>(bindings: I) -> Vec<(String, TraceValue)>
where
    I: IntoIterator<Item = (String, MettaValue)>,
{
    bindings
        .into_iter()
        .map(|(name, val)| (name, trace_value(&val)))
        .collect()
}

/// Snapshot `GenericBindings<MettaValue>` as `(name, TraceValue)` pairs,
/// borrowing only (no ownership transfer, no clones of values themselves).
pub fn trace_bindings_ref(
    bindings: &crate::backend::models::GenericBindings<MettaValue>,
) -> Vec<(String, TraceValue)> {
    bindings
        .iter()
        .map(|(name, val)| (name.to_string(), trace_value(val)))
        .collect()
}

/// Snapshot a slice of `BoundValue = (MettaValue, GenericBindings<MettaValue>)`
/// into a `Vec<BoundValueSnapshot>` suitable for `ContinuationEnter`,
/// `ContinuationEmit`, and related v5 events. Used by the eval-trace
/// binding-flow instrumentation in `eval_loop::process_continuation`.
pub fn trace_bound_values(
    bvs: &[(
        MettaValue,
        crate::backend::models::GenericBindings<MettaValue>,
    )],
) -> Vec<trace_format::BoundValueSnapshot> {
    bvs.iter()
        .map(|(v, b)| trace_format::BoundValueSnapshot {
            value: trace_value(v),
            bindings: trace_bindings_ref(b),
        })
        .collect()
}

#[cfg(test)]
mod convert_tests {
    use super::*;
    use crate::backend::models::metta_value::MettaValue;

    #[test]
    fn test_trace_value_atom() {
        let val = MettaValue::Atom("hello");
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::Atom("hello".to_string()));
    }

    #[test]
    fn test_trace_value_bool() {
        let val = MettaValue::Bool(true);
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::Bool(true));
    }

    #[test]
    fn test_trace_value_long() {
        let val = MettaValue::Long(42);
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::Long(42));
    }

    #[test]
    fn test_trace_value_float() {
        let val = MettaValue::Float(3.14);
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::Float(3.14));
    }

    #[test]
    fn test_trace_value_string() {
        let val = MettaValue::String("world");
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::String("world".to_string()));
    }

    #[test]
    fn test_trace_value_unit() {
        let val = MettaValue::Unit();
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::Unit);
    }

    #[test]
    fn test_trace_value_empty() {
        let val = MettaValue::Empty();
        let tv = trace_value(&val);
        assert_eq!(tv, TraceValue::Empty);
    }

    #[test]
    fn test_trace_value_sexpr() {
        let val = MettaValue::SExpr(vec![
            MettaValue::Atom("+"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let tv = trace_value(&val);
        assert_eq!(
            tv,
            TraceValue::SExpr(vec![
                TraceValue::Atom("+".to_string()),
                TraceValue::Long(1),
                TraceValue::Long(2),
            ])
        );
    }

    #[test]
    fn test_trace_value_nested_sexpr() {
        let inner = MettaValue::SExpr(vec![MettaValue::Atom("*"), MettaValue::Long(3)]);
        let outer = MettaValue::SExpr(vec![MettaValue::Atom("+"), inner, MettaValue::Long(1)]);
        let tv = trace_value(&outer);
        assert_eq!(
            tv,
            TraceValue::SExpr(vec![
                TraceValue::Atom("+".to_string()),
                TraceValue::SExpr(vec![TraceValue::Atom("*".to_string()), TraceValue::Long(3),]),
                TraceValue::Long(1),
            ])
        );
    }

    #[test]
    fn test_trace_value_error() {
        // HE-bisimilar Error(offending=Unit, detail="oops"). The trace
        // converter extracts the message string from the detail slot and
        // keeps the offending value as the child.
        let val = MettaValue::Error(MettaValue::Unit(), MettaValue::String("oops"));
        let tv = trace_value(&val);
        assert_eq!(
            tv,
            TraceValue::Error("oops".to_string(), Box::new(TraceValue::Unit))
        );
    }

    #[test]
    fn test_trace_value_quoted() {
        let val = MettaValue::Quoted(MettaValue::Atom("x"));
        let tv = trace_value(&val);
        assert_eq!(
            tv,
            TraceValue::Quoted(Box::new(TraceValue::Atom("x".to_string())))
        );
    }

    #[test]
    fn test_trace_value_type() {
        let val = MettaValue::Type(MettaValue::Atom("Number"));
        let tv = trace_value(&val);
        assert_eq!(
            tv,
            TraceValue::Type(Box::new(TraceValue::Atom("Number".to_string())))
        );
    }

    #[test]
    fn test_file_table_intern() {
        let mut ft = FileTable::new();
        let id0 = ft.intern("foo.metta");
        let id1 = ft.intern("bar.metta");
        let id0_again = ft.intern("foo.metta");
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id0_again, 0);
        assert_eq!(ft.paths(), &["foo.metta", "bar.metta"]);
    }

    #[test]
    fn test_trace_span_conversion() {
        use crate::ir::{Position, Span};
        let span = Span::new(Position::new(5, 10, 100), Position::new(5, 20, 110));
        let ts = trace_span(&span, 3);
        assert_eq!(ts.file_id, 3);
        assert_eq!(ts.start_row, 5);
        assert_eq!(ts.start_col, 10);
        assert_eq!(ts.end_row, 5);
        assert_eq!(ts.end_col, 20);
    }
}
