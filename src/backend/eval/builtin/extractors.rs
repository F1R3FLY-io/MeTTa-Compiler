//! Value extraction helpers for built-in operations.
//!
//! This module provides helper functions for extracting typed values from MettaValue,
//! with appropriate error messages when types don't match.

use std::sync::Arc;

use crate::backend::models::MettaValue;

/// Extract a Long (integer) value from MettaValue, returning a formatted error if not a Long
pub(super) fn extract_long(value: &MettaValue, context: &str) -> Result<i64, MettaValue> {
    match value {
        MettaValue::Long(n) => Ok(*n),
        other => Err(MettaValue::Error(
            format!(
                "{}: expected Number (integer), got {}",
                context,
                other.friendly_type_name()
            ),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        )),
    }
}

/// Extract a Bool value from MettaValue, returning a formatted error if not a Bool
pub(super) fn extract_bool(value: &MettaValue, context: &str) -> Result<bool, MettaValue> {
    match value {
        MettaValue::Bool(b) => Ok(*b),
        other => Err(MettaValue::Error(
            format!(
                "{}: expected Bool, got {}",
                context,
                other.friendly_type_name()
            ),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        )),
    }
}

/// Extract a Float value from MettaValue, returning a formatted error if not a Float or Long
/// Accepts both Float and Long (converts Long to Float)
pub(super) fn extract_float(value: &MettaValue, context: &str) -> Result<f64, MettaValue> {
    match value {
        MettaValue::Float(f) => Ok(*f),
        MettaValue::Long(n) => Ok(*n as f64),
        other => Err(MettaValue::Error(
            format!(
                "{}: expected Number (float or integer), got {}",
                context,
                other.friendly_type_name()
            ),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        )),
    }
}
