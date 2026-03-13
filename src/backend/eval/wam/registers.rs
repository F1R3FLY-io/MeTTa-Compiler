//! WAM Registers: Argument registers for inter-instruction value passing.
//!
//! The WAM register file holds argument values during pattern matching.
//! When matching `(f (g $x) $y)`, the register file is used to decompose
//! the S-expression tree:
//!
//! ```text
//! A0 = (f (g $x) $y)     ← input expression
//! GetArg A0, 0, A1       → A1 = f
//! GetArg A0, 1, A2       → A2 = (g $x)
//! GetArg A0, 2, A3       → A3 = $y
//! GetArg A2, 0, A4       → A4 = g
//! GetArg A2, 1, A5       → A5 = $x
//! ```
//!
//! This replaces repeated `MatchPath::navigate()` calls that re-traverse
//! the tree from the root for each structural check.

use crate::backend::models::MettaValue;

/// Maximum number of argument registers.
///
/// 16 registers cover typical MeTTa arity (most expressions have <8 children)
/// plus decomposition temporaries. For PLN rules, max observed arity is 6.
pub const MAX_REGISTERS: usize = 16;

/// WAM argument registers for value passing between instructions.
///
/// Register A0 always holds the input expression being matched.
/// Subsequent registers hold decomposed sub-expressions.
#[derive(Clone, Debug)]
pub struct WamRegisters {
    /// Argument registers A0..A15.
    pub args: [MettaValue; MAX_REGISTERS],
    /// Number of valid registers (only args[0..arity] are meaningful).
    pub arity: u8,
    /// Scratch register for temporary computations.
    pub scratch: MettaValue,
}

impl WamRegisters {
    /// Create a new register file with all registers set to UNIT.
    pub fn new() -> Self {
        WamRegisters {
            args: [MettaValue::inline_unit(); MAX_REGISTERS],
            arity: 0,
            scratch: MettaValue::inline_unit(),
        }
    }

    /// Load the input expression into register A0.
    #[inline]
    pub fn load_input(&mut self, value: MettaValue) {
        self.args[0] = value;
        self.arity = 1;
    }

    /// Get the value in register `reg`.
    ///
    /// # Panics
    /// Panics in debug mode if `reg >= MAX_REGISTERS`.
    #[inline]
    pub fn get(&self, reg: u8) -> MettaValue {
        debug_assert!(
            (reg as usize) < MAX_REGISTERS,
            "register index {} out of bounds",
            reg
        );
        self.args[reg as usize]
    }

    /// Set the value in register `reg`.
    ///
    /// # Panics
    /// Panics in debug mode if `reg >= MAX_REGISTERS`.
    #[inline]
    pub fn set(&mut self, reg: u8, value: MettaValue) {
        debug_assert!(
            (reg as usize) < MAX_REGISTERS,
            "register index {} out of bounds",
            reg
        );
        self.args[reg as usize] = value;
        // Track the highest used register
        if reg >= self.arity {
            self.arity = reg + 1;
        }
    }

    /// Reset all registers to UNIT.
    pub fn reset(&mut self) {
        for reg in &mut self.args {
            *reg = MettaValue::inline_unit();
        }
        self.arity = 0;
        self.scratch = MettaValue::inline_unit();
    }

    /// Collect all valid register values for GC root reporting.
    pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
        for i in 0..self.arity as usize {
            let val = self.args[i];
            if val != MettaValue::inline_unit() && val != MettaValue::inline_empty() {
                out.push(val);
            }
        }
        if self.scratch != MettaValue::inline_unit() && self.scratch != MettaValue::inline_empty() {
            out.push(self.scratch);
        }
    }
}

impl Default for WamRegisters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_new_registers() {
        let regs = WamRegisters::new();
        assert_eq!(regs.arity, 0);
        assert_eq!(regs.scratch, MettaValue::inline_unit());
    }

    #[test]
    fn test_load_input() {
        let mut regs = WamRegisters::new();
        regs.load_input(MettaValue::Long(42));
        assert_eq!(regs.get(0), MettaValue::Long(42));
        assert_eq!(regs.arity, 1);
    }

    #[test]
    fn test_set_and_get() {
        let mut regs = WamRegisters::new();
        regs.set(0, MettaValue::Long(1));
        regs.set(3, MettaValue::Long(4));
        regs.set(7, MettaValue::Long(8));

        assert_eq!(regs.get(0), MettaValue::Long(1));
        assert_eq!(regs.get(3), MettaValue::Long(4));
        assert_eq!(regs.get(7), MettaValue::Long(8));
        assert_eq!(regs.arity, 8); // highest register + 1
    }

    #[test]
    fn test_reset() {
        let mut regs = WamRegisters::new();
        regs.set(0, MettaValue::Long(42));
        regs.set(5, MettaValue::Long(99));
        regs.scratch = MettaValue::Long(7);

        regs.reset();
        assert_eq!(regs.arity, 0);
        assert_eq!(regs.get(0), MettaValue::inline_unit());
        assert_eq!(regs.get(5), MettaValue::inline_unit());
        assert_eq!(regs.scratch, MettaValue::inline_unit());
    }

    #[test]
    fn test_gc_roots() {
        let mut regs = WamRegisters::new();
        regs.set(0, MettaValue::Long(10));
        regs.set(1, MettaValue::inline_unit()); // UNIT is excluded
        regs.set(2, MettaValue::Long(30));
        regs.scratch = MettaValue::Long(99);

        let mut roots = Vec::new();
        regs.collect_gc_roots(&mut roots);
        // Should include: Long(10), Long(30), Long(99)
        // UNIT registers are excluded
        assert_eq!(roots.len(), 3);
        assert!(roots.contains(&MettaValue::Long(10)));
        assert!(roots.contains(&MettaValue::Long(30)));
        assert!(roots.contains(&MettaValue::Long(99)));
    }
}
