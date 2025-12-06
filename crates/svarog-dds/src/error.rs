//! Error types for DDS handling.

use thiserror::Error;

/// Errors that can occur when working with DDS files.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Common library error.
    #[error("{0}")]
    Common(#[from] svarog_common::Error),

    /// Invalid DDS magic.
    #[error("invalid DDS magic: expected 'DDS ', got {0:?}")]
    InvalidMagic([u8; 4]),

    /// Invalid DDS header.
    #[error("invalid DDS header: {0}")]
    InvalidHeader(String),

    /// Mipmap size mismatch.
    #[error("mipmap size mismatch: expected {expected}, got {actual}")]
    MipmapSizeMismatch { expected: usize, actual: usize },
}

/// Result type for DDS operations.
pub type Result<T> = std::result::Result<T, Error>;
