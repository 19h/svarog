//! ZIP format structures.
//!
//! This module contains the low-level structures for parsing ZIP archives,
//! including ZIP64 extensions.

pub mod central_dir;
pub mod central_dir_v2;
mod eocd;
pub mod eocd_v2;
mod local;

pub use central_dir::{
    CentralDirectoryHeader, P4kEncryptionExtraField, P4kSha256ExtraField, P4kSignatureExtraField,
    P4kZip64ExtraField,
};
pub use central_dir_v2::{
    CentralDirectoryHeaderV2, CDR_V2_ENTRY_SIZE, CDR_V2_OFFSET_TO_FILENAME_OFFSET,
};
pub use eocd::{Eocd64Locator, Eocd64Record, EocdRecord};
pub use eocd_v2::{Eocd2Record, EOCD_V2_MAGIC, EOCD_V2_SIZE, EOCD_V2_VERSION};
pub use local::LocalFileHeader;

/// Compression methods used in P4K archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CompressionMethod {
    /// No compression (stored).
    Store = 0,
    /// DEFLATE compression.
    Deflate = 8,
    /// Zstandard compression using the ZIP APPNOTE method id.
    ZstdDeprecated = 93,
    /// Zstandard compression (Star Citizen custom).
    Zstd = 100,
    /// Xbox/zlib-wrapped DEFLATE compression.
    DeflateZlib = 101,
}

impl TryFrom<u16> for CompressionMethod {
    type Error = u16;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Store),
            8 => Ok(Self::Deflate),
            93 => Ok(Self::ZstdDeprecated),
            100 => Ok(Self::Zstd),
            101 => Ok(Self::DeflateZlib),
            other => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_layout_matches_zip_and_p4k_reader() {
        assert_eq!(std::mem::size_of::<LocalFileHeader>(), 26);
        assert_eq!(std::mem::size_of::<CentralDirectoryHeader>(), 42);
        assert_eq!(std::mem::size_of::<EocdRecord>(), 18);
        assert_eq!(std::mem::size_of::<Eocd64Locator>(), 16);
        assert_eq!(std::mem::size_of::<Eocd64Record>(), 52);
        assert_eq!(std::mem::size_of::<P4kZip64ExtraField>(), 0x20);
        assert_eq!(std::mem::size_of::<P4kSignatureExtraField>(), 0x84);
        assert_eq!(std::mem::size_of::<P4kEncryptionExtraField>(), 0x06);
        assert_eq!(std::mem::size_of::<P4kSha256ExtraField>(), 0x24);
        assert_eq!(std::mem::size_of::<CompressionMethod>(), 2);
        assert_eq!(std::mem::size_of::<Eocd2Record>(), EOCD_V2_SIZE);
        assert_eq!(
            std::mem::size_of::<CentralDirectoryHeaderV2>(),
            CDR_V2_ENTRY_SIZE
        );
    }

    #[test]
    fn compression_method_ids_match_cig_dump() {
        assert_eq!(CompressionMethod::Store as u16, 0);
        assert_eq!(CompressionMethod::Deflate as u16, 8);
        assert_eq!(CompressionMethod::ZstdDeprecated as u16, 93);
        assert_eq!(CompressionMethod::Zstd as u16, 100);
        assert_eq!(CompressionMethod::DeflateZlib as u16, 101);

        for method in [
            CompressionMethod::Store,
            CompressionMethod::Deflate,
            CompressionMethod::ZstdDeprecated,
            CompressionMethod::Zstd,
            CompressionMethod::DeflateZlib,
        ] {
            assert_eq!(CompressionMethod::try_from(method as u16), Ok(method));
        }
        assert_eq!(CompressionMethod::try_from(102), Err(102));
    }
}
