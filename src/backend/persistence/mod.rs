// Persistence module for MeTTaTron
//
// This module provides PathMap ACT-based persistence for instant O(1) loading
// of knowledge bases via memory-mapped files.

pub mod term_store;
pub mod snapshot;
pub mod persistent_kb;

pub use term_store::{TermStore, TermStoreStats};
pub use snapshot::{SnapshotMetadata, create_snapshot, load_snapshot};
pub use persistent_kb::{PersistentKB, PersistentKBStats};
