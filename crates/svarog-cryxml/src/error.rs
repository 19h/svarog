//! Error types for CryXmlB parsing and writing.

use thiserror::Error;

/// Errors that can occur when parsing or writing CryXmlB files.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Common library error.
    #[error("{0}")]
    Common(#[from] svarog_common::Error),

    /// Invalid magic bytes (not a CryXmlB file).
    #[error("invalid CryXmlB magic: expected 'CryXmlB\\0', got {actual:?}")]
    InvalidMagic { actual: Vec<u8> },

    /// String table offset out of bounds.
    #[error("string offset {offset} out of bounds (string table size: {size})")]
    StringOffsetOutOfBounds { offset: u32, size: usize },

    /// Node index out of bounds.
    #[error("node index {index} out of bounds (total nodes: {count})")]
    NodeIndexOutOfBounds { index: i32, count: usize },

    /// UTF-8 decoding error.
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    /// XML parsing or writing error.
    #[error("XML error: {0}")]
    Xml(String),
}

/// Result type for CryXmlB operations.
pub type Result<T> = std::result::Result<T, Error>;
