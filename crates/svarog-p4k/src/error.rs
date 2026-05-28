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

    /// V2 EOCDR was found but its internal layout is inconsistent.
    #[error("malformed P4K v2 EOCDR: {0}")]
    MalformedV2Eocdr(String),

    /// V2 CDR contains an entry whose name offset / data offset is
    /// outside the bounds of the archive.
    #[error("malformed P4K v2 CDR entry: {0}")]
    MalformedV2Entry(String),

    /// V1 install block is missing or inconsistent with the CDR.
    #[error("malformed P4K v1 install block: {0}")]
    MalformedV1InstallBlock(String),

    /// V1 CDR entry is internally inconsistent.
    #[error("malformed P4K v1 CDR entry: {0}")]
    MalformedV1Entry(String),

    /// P4K subarchive trailer or CDR metadata is inconsistent.
    #[error("malformed P4K subarchive: {0}")]
    MalformedSubArchive(String),

    /// Entry data CRC does not match metadata.
    #[error("CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}")]
    CrcMismatch { expected: u32, actual: u32 },

    /// Entry raw payload SHA-256 does not match metadata.
    #[error("SHA-256 mismatch: expected {expected:?}, got {actual:?}")]
    Sha256Mismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },

    /// Decompression error.
    #[error("decompression error: {0}")]
    Decompression(String),

    /// Decryption error.
    #[error("decryption error: {0}")]
    Decryption(String),

    /// Encryption error.
    #[error("encryption error: {0}")]
    Encryption(String),

    /// Entry not found.
    #[error("entry not found: {0}")]
    EntryNotFound(String),
}

/// Result type for P4K operations.
pub type Result<T> = std::result::Result<T, Error>;
