//! Value extraction helpers for built-in operations.
//!
//! This module provides helper functions for extracting typed values from MettaValue,
//! with appropriate error messages when types don't match.

use crate::backend::models::{MettaValue, MettaValueInner};

/// Extract a Long (integer) value from MettaValue, returning a formatted error if not a Long
pub(super) fn extract_long(value: &MettaValue, context: &str) -> Result<i64, MettaValue> {
    match value.inner() {
        MettaValueInner::Long(n) => Ok(*n),
        _ => Err(MettaValue::Error(
            format!(
                "{}: expected Number (integer), got {}",
                context,
                value.friendly_type_name()
            ),
            MettaValue::Atom("TypeError".to_string()),
        )),
    }
}

/// Extract a Bool value from MettaValue, returning a formatted error if not a Bool
pub(super) fn extract_bool(value: &MettaValue, context: &str) -> Result<bool, MettaValue> {
    match value.inner() {
        MettaValueInner::Bool(b) => Ok(*b),
        _ => Err(MettaValue::Error(
            format!(
                "{}: expected Bool, got {}",
                context,
                value.friendly_type_name()
            ),
            MettaValue::Atom("TypeError".to_string()),
        )),
    }
}

/// Extract a Float value from MettaValue, returning a formatted error if not a Float or Long
/// Accepts both Float and Long (converts Long to Float)
pub(super) fn extract_float(value: &MettaValue, context: &str) -> Result<f64, MettaValue> {
    match value.inner() {
        MettaValueInner::Float(f) => Ok(*f),
        MettaValueInner::Long(n) => Ok(*n as f64),
        _ => Err(MettaValue::Error(
            format!(
                "{}: expected Number (float or integer), got {}",
                context,
                value.friendly_type_name()
            ),
            MettaValue::Atom("TypeError".to_string()),
        )),
    }
}
