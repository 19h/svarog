//! Error types for DataCore parsing.

use thiserror::Error;

/// Errors that can occur when working with DataCore databases.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Common library error.
    #[error("{0}")]
    Common(#[from] svarog_common::Error),

    /// Unsupported database version.
    #[error("unsupported DataCore version: {0} (expected 5 or 6)")]
    UnsupportedVersion(u32),

    /// String offset out of bounds.
    #[error("string offset {offset} out of bounds (table size: {size})")]
    StringOffsetOutOfBounds { offset: i32, size: usize },

    /// Invalid struct index.
    #[error("invalid struct index: {index} (total: {count})")]
    InvalidStructIndex { index: i32, count: usize },

    /// Invalid record GUID.
    #[error("record not found: {0}")]
    RecordNotFound(String),

    /// Invalid data type.
    #[error("invalid data type: {0}")]
    InvalidDataType(u16),

    /// Export error.
    #[error("export error: {0}")]
    Export(String),
}

/// Result type for DataCore operations.
pub type Result<T> = std::result::Result<T, Error>;
