//! Error types for the P4K crate.

use thiserror::Error;

/// Errors that can occur when working with P4K archives.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Common library error.
    #[error("{0}")]
    Common(#[from] svarog_common::Error),

    /// Invalid ZIP magic bytes.
    #[error("invalid ZIP signature: expected {expected:#010x}, got {actual:#010x}")]
    InvalidSignature { expected: u32, actual: u32 },

    /// Could not find the end of central directory record.
    #[error("could not find end of central directory record")]
    EocdNotFound,

    /// ZIP64 record not found when expected.
    #[error("ZIP64 end of central directory not found")]
    Zip64EocdNotFound,

    /// Invalid extra field ID.
    #[error("invalid extra field ID: expected {expected:#06x}, got {actual:#06x}")]
    InvalidExtraFieldId { expected: u16, actual: u16 },

    /// Unsupported compression method.
    #[error("unsupported compression method: {0}")]
    UnsupportedCompression(u16),

    /// Unsupported version.
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u16),

    /// Decompression error.
    #[error("decompression error: {0}")]
    Decompression(String),

    /// Decryption error.
    #[error("decryption error: {0}")]
    Decryption(String),

    /// Entry not found.
    #[error("entry not found: {0}")]
    EntryNotFound(String),
}

/// Result type for P4K operations.
pub type Result<T> = std::result::Result<T, Error>;
