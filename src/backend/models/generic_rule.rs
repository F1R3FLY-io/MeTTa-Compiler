//! Generic Rule Types for Zero-Conversion Evaluation.
//!
//! This module provides generic rule types that work with any value type
//! implementing `MettaValueTrait`:
//!
//! - `GenericRule<V>` - Rule with generic value type for evaluation
//! - `RuleBytes` - Serialized rule for type-agnostic storage
//!
//! ## Design
//!
//! Rules can be stored in two forms:
//!
//! 1. **Generic Form** (`GenericRule<V>`) - For evaluation with native value type
//! 2. **Byte Form** (`RuleBytes`) - For storage in PathMap/MORK
//!
//! The byte form enables deserializing rules into any value type on demand,
//! while the generic form provides zero-conversion evaluation.
//!
//! ## Zero-Conversion Pattern
//!
//! ```ignore
//! // Rule storage (bytes)
//! let rule_bytes = RuleBytes::from_rule(&generic_rule);
//!
//! // Deserialize to any value type
//! let heap_rule: GenericRule<MettaValue> = rule_bytes.to_rule(&heap_factory);
//! let arena_rule: GenericRule<MettaValue> = rule_bytes.to_rule(&arena_factory);
//! ```

use crate::backend::models::metta_value_trait::{MettaValueTrait, MettaValueFactory};

/// Generic rule type that works with any value type.
///
/// This is parameterized over `V: MettaValueTrait` to enable zero-conversion
/// evaluation with both heap and arena allocation strategies.
#[derive(Debug, Clone)]
pub struct GenericRule<V: MettaValueTrait> {
    /// Left-hand side pattern
    pub lhs: V,
    /// Right-hand side template
    pub rhs: V,
    /// Cached index into multiplicity counts array.
    /// Set during add_rule(), used for O(1) count lookup.
    /// None for rules created before being added to an environment.
    pub(crate) multiplicity_idx: Option<u32>,
}

impl<V: MettaValueTrait> GenericRule<V> {
    /// Create a new rule from values
    pub fn new(lhs: V, rhs: V) -> Self {
        GenericRule {
            lhs,
            rhs,
            multiplicity_idx: None,
        }
    }

    /// Create a new rule from values (alias for API compatibility)
    #[inline]
    pub fn from_arc(lhs: V, rhs: V) -> Self {
        GenericRule::new(lhs, rhs)
    }

    /// Create rule with pre-assigned index (for bulk operations)
    #[allow(dead_code)]
    pub(crate) fn with_index(lhs: V, rhs: V, idx: u32) -> Self {
        GenericRule {
            lhs,
            rhs,
            multiplicity_idx: Some(idx),
        }
    }

    /// Create rule from values with pre-assigned index (alias for API compatibility)
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn from_arc_with_index(lhs: V, rhs: V, idx: u32) -> Self {
        GenericRule::with_index(lhs, rhs, idx)
    }

    /// Get a reference to the LHS pattern
    #[inline]
    pub fn lhs_ref(&self) -> &V {
        &self.lhs
    }

    /// Get a reference to the RHS template
    #[inline]
    pub fn rhs_ref(&self) -> &V {
        &self.rhs
    }

    /// Clone the RHS
    #[inline]
    pub fn rhs_arc(&self) -> V {
        self.rhs.clone()
    }

    /// Clone the LHS
    #[inline]
    pub fn lhs_arc(&self) -> V {
        self.lhs.clone()
    }

    /// Extract the head symbol from the LHS pattern for indexing
    #[inline]
    pub fn get_head_symbol(&self) -> Option<&str> {
        self.lhs.get_head_symbol()
    }

    /// Get the arity (number of arguments) of the LHS pattern
    #[inline]
    pub fn get_arity(&self) -> usize {
        self.lhs.get_arity()
    }
}

/// Serialized rule for type-agnostic storage.
///
/// Rules are stored as bytes (serialized via `MettaValueTrait::serialize()`)
/// and can be deserialized into any value type on demand.
///
/// This enables zero-conversion storage where rules are stored once as bytes
/// and deserialized into the evaluation's native value type.
#[derive(Debug, Clone)]
pub struct RuleBytes {
    /// Serialized LHS pattern
    pub lhs_bytes: Vec<u8>,
    /// Serialized RHS template
    pub rhs_bytes: Vec<u8>,
    /// Cached multiplicity index (preserved through serialization)
    pub multiplicity_idx: Option<u32>,
}

impl RuleBytes {
    /// Create a RuleBytes from a generic rule.
    ///
    /// Serializes both LHS and RHS to bytes for storage.
    pub fn from_rule<V: MettaValueTrait>(rule: &GenericRule<V>) -> Self {
        RuleBytes {
            lhs_bytes: rule.lhs.serialize(),
            rhs_bytes: rule.rhs.serialize(),
            multiplicity_idx: rule.multiplicity_idx,
        }
    }

    /// Deserialize to a generic rule using the given factory.
    ///
    /// This enables zero-conversion pattern: store once as bytes,
    /// deserialize to any value type on demand.
    pub fn to_rule<V: MettaValueTrait, F: MettaValueFactory<V>>(
        &self,
        factory: &F,
    ) -> Result<GenericRule<V>, String> {
        let (lhs, _) = factory.deserialize(&self.lhs_bytes)?;
        let (rhs, _) = factory.deserialize(&self.rhs_bytes)?;
        Ok(GenericRule {
            lhs,
            rhs,
            multiplicity_idx: self.multiplicity_idx,
        })
    }

    /// Serialize the RuleBytes to a single byte vector.
    ///
    /// Format:
    /// - 4 bytes: multiplicity_idx (0xFFFFFFFF if None)
    /// - 4 bytes: lhs_bytes length
    /// - lhs_bytes
    /// - 4 bytes: rhs_bytes length
    /// - rhs_bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes =
            Vec::with_capacity(12 + self.lhs_bytes.len() + self.rhs_bytes.len());

        // Multiplicity index (4 bytes)
        let idx = self.multiplicity_idx.unwrap_or(0xFFFFFFFF);
        bytes.extend_from_slice(&idx.to_le_bytes());

        // LHS bytes
        bytes.extend_from_slice(&(self.lhs_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.lhs_bytes);

        // RHS bytes
        bytes.extend_from_slice(&(self.rhs_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.rhs_bytes);

        bytes
    }

    /// Deserialize a RuleBytes from a byte slice.
    ///
    /// Returns the RuleBytes and the number of bytes consumed.
    pub fn from_bytes(bytes: &[u8]) -> Result<(Self, usize), String> {
        if bytes.len() < 12 {
            return Err("RuleBytes: insufficient bytes".to_string());
        }

        let mut offset = 0;

        // Multiplicity index
        let idx_bytes: [u8; 4] = bytes[offset..offset + 4]
            .try_into()
            .map_err(|_| "RuleBytes: failed to read multiplicity_idx")?;
        let idx = u32::from_le_bytes(idx_bytes);
        let multiplicity_idx = if idx == 0xFFFFFFFF { None } else { Some(idx) };
        offset += 4;

        // LHS bytes
        let lhs_len_bytes: [u8; 4] = bytes[offset..offset + 4]
            .try_into()
            .map_err(|_| "RuleBytes: failed to read lhs length")?;
        let lhs_len = u32::from_le_bytes(lhs_len_bytes) as usize;
        offset += 4;

        if bytes.len() < offset + lhs_len + 4 {
            return Err("RuleBytes: insufficient bytes for lhs".to_string());
        }
        let lhs_bytes = bytes[offset..offset + lhs_len].to_vec();
        offset += lhs_len;

        // RHS bytes
        let rhs_len_bytes: [u8; 4] = bytes[offset..offset + 4]
            .try_into()
            .map_err(|_| "RuleBytes: failed to read rhs length")?;
        let rhs_len = u32::from_le_bytes(rhs_len_bytes) as usize;
        offset += 4;

        if bytes.len() < offset + rhs_len {
            return Err("RuleBytes: insufficient bytes for rhs".to_string());
        }
        let rhs_bytes = bytes[offset..offset + rhs_len].to_vec();
        offset += rhs_len;

        Ok((
            RuleBytes {
                lhs_bytes,
                rhs_bytes,
                multiplicity_idx,
            },
            offset,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_generic_rule_new() {
        let lhs = MettaValue::Atom("foo".to_string());
        let rhs = MettaValue::Long(42);
        let rule: GenericRule<MettaValue> = GenericRule::new(lhs.clone(), rhs.clone());

        assert_eq!(rule.lhs, lhs);
        assert_eq!(rule.rhs, rhs);
        assert!(rule.multiplicity_idx.is_none());
    }

    #[test]
    fn test_generic_rule_with_index() {
        let lhs = MettaValue::Atom("foo".to_string());
        let rhs = MettaValue::Long(42);
        let rule: GenericRule<MettaValue> = GenericRule::with_index(lhs.clone(), rhs.clone(), 5);

        assert_eq!(rule.lhs, lhs);
        assert_eq!(rule.rhs, rhs);
        assert_eq!(rule.multiplicity_idx, Some(5));
    }

    #[test]
    fn test_generic_rule_accessors() {
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);
        let rhs = MettaValue::Long(42);
        let rule: GenericRule<MettaValue> = GenericRule::new(lhs, rhs);

        assert_eq!(rule.get_head_symbol(), Some("add"));
        assert_eq!(rule.get_arity(), 2);
    }

    #[test]
    fn test_rule_bytes_roundtrip() {
        let factory = GcFactory::default();
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);
        let rhs = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);
        let rule: GenericRule<MettaValue> =
            GenericRule::with_index(lhs.clone(), rhs.clone(), 10);

        // Serialize to bytes
        let rule_bytes = RuleBytes::from_rule(&rule);

        // Deserialize back
        let restored: GenericRule<MettaValue> = rule_bytes
            .to_rule(&factory)
            .expect("should deserialize");

        assert_eq!(restored.lhs, lhs);
        assert_eq!(restored.rhs, rhs);
        assert_eq!(restored.multiplicity_idx, Some(10));
    }

    #[test]
    fn test_rule_bytes_to_from_bytes() {
        let lhs = MettaValue::Atom("foo".to_string());
        let rhs = MettaValue::Long(42);
        let rule: GenericRule<MettaValue> = GenericRule::with_index(lhs, rhs, 7);

        // Create RuleBytes
        let rule_bytes = RuleBytes::from_rule(&rule);

        // Serialize to single byte vector
        let bytes = rule_bytes.to_bytes();

        // Deserialize back
        let (restored_bytes, consumed) =
            RuleBytes::from_bytes(&bytes).expect("should deserialize");

        assert_eq!(consumed, bytes.len());
        assert_eq!(restored_bytes.lhs_bytes, rule_bytes.lhs_bytes);
        assert_eq!(restored_bytes.rhs_bytes, rule_bytes.rhs_bytes);
        assert_eq!(restored_bytes.multiplicity_idx, rule_bytes.multiplicity_idx);
    }

    #[test]
    fn test_rule_bytes_none_multiplicity() {
        let lhs = MettaValue::Atom("foo".to_string());
        let rhs = MettaValue::Long(42);
        let rule: GenericRule<MettaValue> = GenericRule::new(lhs, rhs);

        let rule_bytes = RuleBytes::from_rule(&rule);
        assert!(rule_bytes.multiplicity_idx.is_none());

        // Roundtrip through bytes
        let bytes = rule_bytes.to_bytes();
        let (restored, _) = RuleBytes::from_bytes(&bytes).expect("should deserialize");
        assert!(restored.multiplicity_idx.is_none());
    }
}
