//! Error types for svarog-common.

use thiserror::Error;

/// Common error type for Svarog operations.
#[derive(Debug, Error)]
pub enum Error {
    /// End of buffer reached while reading.
    #[error("unexpected end of buffer: needed {needed} bytes but only {available} available")]
    UnexpectedEof { needed: usize, available: usize },

    /// Invalid magic bytes encountered.
    #[error("invalid magic: expected {expected:?}, got {actual:?}")]
    InvalidMagic {
        expected: Vec<u8>,
        actual: Vec<u8>,
    },

    /// Value did not match expected.
    #[error("expected value {expected}, got {actual}")]
    ExpectedValue { expected: String, actual: String },

    /// Invalid GUID format.
    #[error("invalid GUID format: {0}")]
    InvalidGuid(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// UTF-8 decoding error.
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    /// Missing null terminator in string.
    #[error("string missing null terminator")]
    MissingNullTerminator,
}

/// Result type alias using the common Error type.
pub type Result<T> = std::result::Result<T, Error>;
