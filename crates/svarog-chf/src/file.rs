//! CHF file handling.

use std::fs;
use std::io::Read;
use std::path::Path;

use svarog_common::{crc, BinaryReader};

use crate::{Error, Result};

/// The fixed size of a CHF file in bytes.
pub const CHF_SIZE: usize = 4096;

/// CIG magic bytes at the start of CHF files.
const CIG_MAGIC: u16 = 0x4242;

/// Magic bytes indicating a modded character file.
const MODDED_MAGIC: &[u8; 8] = b"diogotr7";

/// A Star Citizen character head file.
///
/// CHF files contain character customization data including DNA (facial features),
/// materials, and equipped items.
#[derive(Debug, Clone)]
pub struct ChfFile {
    /// The decompressed character data.
    data: Vec<u8>,
    /// Whether this is a modded character.
    modded: bool,
}

impl ChfFile {
    /// Create a new CHF file from raw (uncompressed) data.
    pub fn new(data: Vec<u8>, modded: bool) -> Self {
        Self { data, modded }
    }

    /// Get the decompressed data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get mutable access to the data.
    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.data
    }

    /// Check if this is a modded character.
    pub fn is_modded(&self) -> bool {
        self.modded
    }

    /// Set the modded flag.
    pub fn set_modded(&mut self, modded: bool) {
        self.modded = modded;
    }

    /// Read a CHF file from disk.
    pub fn from_chf<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Validate extension
        if path.extension().and_then(|e| e.to_str()) != Some("chf") {
            return Err(Error::InvalidExtension {
                expected: "chf".to_string(),
                actual: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }

        let file_bytes = fs::read(path)?;

        if file_bytes.len() != CHF_SIZE {
            return Err(Error::InvalidSize(file_bytes.len()));
        }

        Self::parse(&file_bytes)
    }

    /// Read from a bin file (uncompressed).
    pub fn from_bin<P: AsRef<Path>>(path: P, modded: bool) -> Result<Self> {
        let path = path.as_ref();

        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            return Err(Error::InvalidExtension {
                expected: "bin".to_string(),
                actual: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }

        let data = fs::read(path)?;
        Ok(Self::new(data, modded))
    }

    /// Parse CHF data from bytes.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() != CHF_SIZE {
            return Err(Error::InvalidSize(data.len()));
        }

        let mut reader = BinaryReader::new(data);

        // Read and validate magic
        let magic = reader.read_u16()?;
        if magic != CIG_MAGIC {
            return Err(Error::InvalidMagic(magic));
        }

        // Skip unknown bytes (possibly version)
        reader.advance(2);

        // Read header values
        let expected_crc = reader.read_u32()?;
        let compressed_size = reader.read_u32()? as usize;
        let uncompressed_size = reader.read_u32()? as usize;

        // Validate CRC32C (covers everything after the CRC field)
        let crc_data = &data[16..];
        let actual_crc = crc::hash_bytes(crc_data);

        if actual_crc != expected_crc {
            return Err(Error::CrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        // Decompress data
        let compressed_data = reader.read_bytes(compressed_size)?;
        let mut decompressed = Vec::with_capacity(uncompressed_size);

        zstd::Decoder::new(compressed_data)
            .map_err(|e| Error::Decompression(e.to_string()))?
            .read_to_end(&mut decompressed)
            .map_err(|e| Error::Decompression(e.to_string()))?;

        if decompressed.len() != uncompressed_size {
            return Err(Error::SizeMismatch {
                expected: uncompressed_size,
                actual: decompressed.len(),
            });
        }

        // Check for modded magic at the end
        let trailer = &data[CHF_SIZE - 8..];
        let is_modded = Self::check_modded(trailer);

        Ok(Self {
            data: decompressed,
            modded: is_modded,
        })
    }

    /// Write to a CHF file.
    pub fn write_to_chf<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        if path.extension().and_then(|e| e.to_str()) != Some("chf") {
            return Err(Error::InvalidExtension {
                expected: "chf".to_string(),
                actual: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }

        let buffer = self.to_chf_bytes()?;
        fs::write(path, buffer)?;
        Ok(())
    }

    /// Write to a bin file (uncompressed).
    pub fn write_to_bin<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            return Err(Error::InvalidExtension {
                expected: "bin".to_string(),
                actual: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }

        fs::write(path, &self.data)?;
        Ok(())
    }

    /// Convert to CHF bytes for writing.
    pub fn to_chf_bytes(&self) -> Result<Vec<u8>> {
        let mut output = vec![0u8; CHF_SIZE];

        // Compress data
        let compressed =
            zstd::encode_all(&self.data[..], 16).map_err(|e| Error::Compression(e.to_string()))?;

        // Check if it fits
        if 16 + compressed.len() > CHF_SIZE - 8 {
            return Err(Error::Compression(
                "compressed data too large for CHF file".to_string(),
            ));
        }

        // Write header
        output[0..2].copy_from_slice(&CIG_MAGIC.to_le_bytes());
        // Bytes 2-3 are unknown, leave as zero
        // Bytes 4-7 are CRC, will be filled in later
        output[8..12].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
        output[12..16].copy_from_slice(&(self.data.len() as u32).to_le_bytes());

        // Write compressed data
        output[16..16 + compressed.len()].copy_from_slice(&compressed);

        // Write modded magic if applicable
        if self.modded {
            output[CHF_SIZE - 8..].copy_from_slice(MODDED_MAGIC);
        }

        // Calculate and write CRC
        let crc = crc::hash_bytes(&output[16..]);
        output[4..8].copy_from_slice(&crc.to_le_bytes());

        Ok(output)
    }

    /// Check if the trailer indicates a modded file.
    fn check_modded(trailer: &[u8]) -> bool {
        // Modded if ends with our magic or all zeros
        trailer == MODDED_MAGIC || trailer == [0u8; 8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_modded() {
        assert!(ChfFile::check_modded(b"diogotr7"));
        assert!(ChfFile::check_modded(&[0u8; 8]));
        assert!(!ChfFile::check_modded(b"12345678"));
    }
}
