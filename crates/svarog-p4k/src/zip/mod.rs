//! ZIP format structures.
//!
//! This module contains the low-level structures for parsing ZIP archives,
//! including ZIP64 extensions.

pub mod central_dir;
mod eocd;
mod local;

pub use central_dir::CentralDirectoryHeader;
pub use eocd::{Eocd64Locator, Eocd64Record, EocdRecord};
pub use local::LocalFileHeader;

/// Compression methods used in P4K archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CompressionMethod {
    /// No compression (stored).
    Store = 0,
    /// DEFLATE compression.
    Deflate = 8,
    /// Zstandard compression (Star Citizen custom).
    Zstd = 100,
}

impl TryFrom<u16> for CompressionMethod {
    type Error = u16;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Store),
            8 => Ok(Self::Deflate),
            100 => Ok(Self::Zstd),
            other => Err(other),
        }
    }
}
