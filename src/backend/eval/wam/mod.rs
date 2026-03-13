//! MeTTa-WAM: Warren Abstract Machine adapted for MeTTa evaluation.
//!
//! This module implements a WAM-inspired evaluation engine that selectively adopts
//! WAM concepts for MeTTa's specific needs:
//!
//! - **Trail-based binding undo**: Eliminates `GenericBindings` cloning on nondeterministic
//!   branches. For the 93% single-match case, the trail is never unwound (zero undo cost).
//! - **Stack-allocated binding frames**: O(1) indexed slot access replaces O(n) name-based
//!   lookup in `GenericBindings`.
//! - **WAM-style compiled instructions**: Replaces StructuralMatcher's repeated path navigation
//!   with register-based decomposition.
//! - **Choice points for all-solutions**: Unlike Prolog's depth-first single-solution model,
//!   MeTTa-WAM explores ALL alternatives and accumulates results.
//!
//! The WAM engine operates as a **rule dispatch accelerator** inside the existing trampoline.
//! It handles pattern matching and binding, but delegates to the trampoline for special forms
//! that it doesn't handle natively (Tier 2 forms).
//!
//! # Architecture
//!
//! ```text
//! MeTTa Source → compile() → MettaValue
//!                              ↓
//!                     eval_trampoline_generic()
//!                              ↓
//!                    ┌── S-expression with rules? ──┐
//!                    │                              │
//!                    ▼                              ▼
//!              WAM dispatch              Existing structural/MORK
//!              (wam_dispatch_rules)       (try_match_all_rules_generic)
//!                    │                              │
//!                    └──── dispatch_rule_matches ───┘
//! ```
//!
//! # References
//!
//! - Warren, D. H. D. (1983). "An Abstract Prolog Instruction Set"
//! - Aït-Kaci, H. (1991). "Warren's Abstract Machine: A Tutorial Reconstruction"

pub mod trail;
pub mod binding_frame;
pub mod choice_point;
pub mod registers;
pub mod instructions;
pub mod compiler;
pub mod engine;

// Re-exports for convenience
pub use trail::{Trail, TrailEntry};
pub use binding_frame::WamBindingFrame;
pub use choice_point::{WamChoicePoint, WamAlternative};
pub use registers::WamRegisters;
pub use instructions::WamInstruction;
pub use compiler::{WamCode, compile_rule_lhs, compile_rule_group};
pub use engine::{wam_dispatch_rules, wam_try_match, WamState};
