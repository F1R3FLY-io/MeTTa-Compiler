mod proptest;

// Re-export the proptest modules so the tests are discovered by cargo test
pub use proptest::compiling;