//! Error types for CHF parsing.

use thiserror::Error;

/// Errors that can occur when working with CHF files.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Common library error.
    #[error("{0}")]
    Common(#[from] svarog_common::Error),

    /// Invalid file extension.
    #[error("invalid file extension: expected {expected}, got {actual}")]
    InvalidExtension { expected: String, actual: String },

    /// Invalid file size.
    #[error("invalid CHF file size: expected 4096 bytes, got {0}")]
    InvalidSize(usize),

    /// Invalid magic bytes.
    #[error("invalid CHF magic: expected 0x4242, got {0:#06x}")]
    InvalidMagic(u16),

    /// CRC32C checksum mismatch.
    #[error("CRC32C mismatch: expected {expected:#010x}, got {actual:#010x}")]
    CrcMismatch { expected: u32, actual: u32 },

    /// Decompression error.
    #[error("decompression error: {0}")]
    Decompression(String),

    /// Compression error.
    #[error("compression error: {0}")]
    Compression(String),

    /// Decompressed size mismatch.
    #[error("decompressed size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: usize, actual: usize },
}

/// Result type for CHF operations.
pub type Result<T> = std::result::Result<T, Error>;
