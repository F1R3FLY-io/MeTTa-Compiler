//! Parse emitter trait and concrete implementations.
//!
//! The `ParseEmitter` trait abstracts over what the parser produces, enabling:
//! - `ValueEmitter<F>`: Hot path — emits `MettaValue` directly via a factory (zero IR)
//! - `IrEmitter`: Cold path — emits `MettaExpr` IR nodes for `--sexpr` mode

use crate::backend::models::{MettaValueFactory, MettaValueTrait};
use crate::ir::{MettaExpr, Position, Span};

/// Trait abstracting parser output. The parser calls these methods to emit
/// parsed constructs, and the emitter decides the representation.
pub trait ParseEmitter {
    type Output;

    fn emit_atom(&mut self, text: &str, span: Span) -> Self::Output;
    fn emit_bool(&mut self, value: bool, span: Span) -> Self::Output;
    fn emit_string(&mut self, text: &str, span: Span) -> Self::Output;
    fn emit_integer(&mut self, value: i64, span: Span) -> Self::Output;
    fn emit_float(&mut self, value: f64, span: Span) -> Self::Output;
    fn emit_sexpr(&mut self, items: Vec<Self::Output>, span: Span) -> Self::Output;
    fn emit_conjunction(&mut self, items: Vec<Self::Output>, span: Span) -> Self::Output;
    fn emit_prefix(&mut self, op: &str, op_span: Span, arg: Self::Output, full_span: Span) -> Self::Output;
}

// ============================================================================
// ValueEmitter — Hot path: direct-to-MettaValue via factory
// ============================================================================

/// Emits `MettaValue` directly via a factory, skipping all intermediate IR.
pub struct ValueEmitter<'f, V, F>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    factory: &'f F,
    _phantom: std::marker::PhantomData<V>,
}

impl<'f, V, F> ValueEmitter<'f, V, F>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    pub fn new(factory: &'f F) -> Self {
        Self {
            factory,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<'f, V, F> ParseEmitter for ValueEmitter<'f, V, F>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    type Output = V;

    #[inline]
    fn emit_atom(&mut self, text: &str, _span: Span) -> V {
        self.factory.atom(text)
    }

    #[inline]
    fn emit_bool(&mut self, value: bool, _span: Span) -> V {
        self.factory.bool(value)
    }

    #[inline]
    fn emit_string(&mut self, text: &str, _span: Span) -> V {
        self.factory.string(text)
    }

    #[inline]
    fn emit_integer(&mut self, value: i64, _span: Span) -> V {
        self.factory.long(value)
    }

    #[inline]
    fn emit_float(&mut self, value: f64, _span: Span) -> V {
        self.factory.float(value)
    }

    #[inline]
    fn emit_sexpr(&mut self, items: Vec<V>, _span: Span) -> V {
        // Detect (quote X) and produce Quoted(X)
        if items.len() == 2 {
            if let Some(name) = items[0].as_atom() {
                if name == "quote" {
                    return self.factory.quote(items.into_iter().nth(1).expect("items has 2 elements"));
                }
            }
        }
        self.factory.sexpr(items)
    }

    #[inline]
    fn emit_conjunction(&mut self, items: Vec<V>, _span: Span) -> V {
        self.factory.conjunction(items)
    }

    #[inline]
    fn emit_prefix(&mut self, op: &str, _op_span: Span, arg: V, _full_span: Span) -> V {
        if op == "quote" {
            self.factory.quote(arg)
        } else {
            self.factory.sexpr(vec![self.factory.atom(op), arg])
        }
    }
}

// ============================================================================
// IrEmitter — Cold path: builds MettaExpr IR nodes
// ============================================================================

/// Emits `MettaExpr` IR nodes with span information for `--sexpr` mode
/// and other consumers that need the IR representation.
pub struct IrEmitter;

impl IrEmitter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IrEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseEmitter for IrEmitter {
    type Output = MettaExpr;

    #[inline]
    fn emit_atom(&mut self, text: &str, span: Span) -> MettaExpr {
        MettaExpr::Atom(text.to_string(), Some(span))
    }

    #[inline]
    fn emit_bool(&mut self, value: bool, span: Span) -> MettaExpr {
        // Booleans are represented as atoms "True"/"False" in IR
        let text = if value { "True" } else { "False" };
        MettaExpr::Atom(text.to_string(), Some(span))
    }

    #[inline]
    fn emit_string(&mut self, text: &str, span: Span) -> MettaExpr {
        MettaExpr::String(text.to_string(), Some(span))
    }

    #[inline]
    fn emit_integer(&mut self, value: i64, span: Span) -> MettaExpr {
        MettaExpr::Integer(value, Some(span))
    }

    #[inline]
    fn emit_float(&mut self, value: f64, span: Span) -> MettaExpr {
        MettaExpr::Float(value, Some(span))
    }

    #[inline]
    fn emit_sexpr(&mut self, items: Vec<MettaExpr>, span: Span) -> MettaExpr {
        MettaExpr::List(items, Some(span))
    }

    #[inline]
    fn emit_conjunction(&mut self, mut items: Vec<MettaExpr>, span: Span) -> MettaExpr {
        // In IR, conjunction is represented as a list with comma prefix: (, item1 item2 ...)
        let mut full = Vec::with_capacity(items.len() + 1);
        full.push(MettaExpr::Atom(",".to_string(), None));
        full.append(&mut items);
        MettaExpr::List(full, Some(span))
    }

    #[inline]
    fn emit_prefix(&mut self, op: &str, op_span: Span, arg: MettaExpr, full_span: Span) -> MettaExpr {
        MettaExpr::List(
            vec![
                MettaExpr::Atom(op.to_string(), Some(op_span)),
                arg,
            ],
            Some(full_span),
        )
    }
}

/// Helper to create a zero-length span at a position
pub fn point_span(line: usize, col: usize, byte_offset: usize) -> Span {
    let pos = Position::new(line, col, byte_offset);
    Span::new(pos, pos)
}
