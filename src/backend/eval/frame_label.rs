//! `FrameLabel` — the human-readable evaluation-frame label used for GC-root
//! frame pushes and stack-trace rendering.
//!
//! ## Why this lives in its own module (A5.0)
//!
//! `FrameLabel` was originally defined in `frame_chain`, the thread-local
//! GC-root chain. Phase A5 routed the index collector to read roots
//! structurally from the typed K-spine
//! ([`cesk::k_spine`](super::cesk::k_spine)) rather than from a discovered
//! frame chain, and F4 deleted the `frame_chain` module outright. `FrameLabel`
//! had already been relocated here — a standalone module — so it survived that
//! deletion with a stable signature for the
//! [`push_expr_vec_frame`](super::expr_vec_frame::push_expr_vec_frame) call
//! sites, which still pass a label the structural reader ignores.

use std::fmt;

/// Human-readable frame label for stack trace rendering.
///
/// Describes the evaluation context of a frame in the chain.
#[derive(Debug, Clone, Copy)]
pub enum FrameLabel {
    /// `include "path"` — loading and evaluating a MeTTa file
    Include,
    /// `import! module` — importing a module into scope
    Import,
    /// `assertEqual actual expected` — assertion evaluation
    AssertEqual,
    /// `assertAlphaEqual actual expected` — alpha-equiv assertion
    AssertAlphaEqual,
    /// `assertEqualToResult actual expected` — result assertion
    AssertEqualToResult,
    /// `assertAlphaEqualToResult actual expected` — alpha result assertion
    AssertAlphaEqualToResult,
    /// Top-level `eval` call
    Eval,
    /// Bytecode VM frame whose live execution stacks (value_stack / locals /
    /// results / current_bindings / choice_points / …) are registered as GC
    /// roots while it calls a NESTED `eval_trampoline` (so a mid-execution
    /// collection inside that inner trampoline cannot free the outer VM's
    /// live values). See `bytecode/vm/mod.rs::with_vm_roots_frame`.
    BytecodeVm,
    /// Extensible custom label
    Custom(&'static str),
}

impl fmt::Display for FrameLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameLabel::Include => write!(f, "include"),
            FrameLabel::Import => write!(f, "import!"),
            FrameLabel::AssertEqual => write!(f, "assertEqual"),
            FrameLabel::AssertAlphaEqual => write!(f, "assertAlphaEqual"),
            FrameLabel::AssertEqualToResult => write!(f, "assertEqualToResult"),
            FrameLabel::AssertAlphaEqualToResult => write!(f, "assertAlphaEqualToResult"),
            FrameLabel::Eval => write!(f, "eval"),
            FrameLabel::BytecodeVm => write!(f, "bytecode-vm"),
            FrameLabel::Custom(s) => write!(f, "{}", s),
        }
    }
}
