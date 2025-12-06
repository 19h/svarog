//! Decompression utilities for P4K archives.

use std::io::Read;

use flate2::read::DeflateDecoder;

use crate::{Error, Result};

/// Decompress Zstandard-compressed data.
pub fn decompress_zstd(data: &[u8], output: &mut Vec<u8>) -> Result<()> {
    let mut decoder = zstd::Decoder::new(data).map_err(|e| Error::Decompression(e.to_string()))?;

    output.clear();
    decoder
        .read_to_end(output)
        .map_err(|e| Error::Decompression(e.to_string()))?;

    Ok(())
}

/// Decompress Zstandard-compressed data with known output size.
pub fn decompress_zstd_sized(data: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected_size);
    decompress_zstd(data, &mut output)?;
    Ok(output)
}

/// Decompress DEFLATE-compressed data.
pub fn decompress_deflate(data: &[u8], output: &mut Vec<u8>) -> Result<()> {
    let mut decoder = DeflateDecoder::new(data);

    output.clear();
    decoder
        .read_to_end(output)
        .map_err(|e| Error::Decompression(e.to_string()))?;

    Ok(())
}

/// Decompress DEFLATE-compressed data with known output size.
pub fn decompress_deflate_sized(data: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected_size);
    decompress_deflate(data, &mut output)?;
    Ok(output)
}

/// Thread-local Zstandard decompressor for efficiency.
///
/// Creating a Zstd decoder has some overhead, so we reuse decoders
/// within each thread. Currently not used but available for parallel extraction.
#[allow(dead_code)]
pub struct ZstdDecompressor {
    // The zstd crate doesn't expose a stateful decompressor directly,
    // but we could use raw FFI if needed for performance.
    // For now, we just create new decoders per call.
}

#[allow(dead_code)]
impl ZstdDecompressor {
    /// Create a new thread-local decompressor.
    pub fn new() -> Self {
        Self {}
    }

    /// Decompress data.
    pub fn decompress(&mut self, data: &[u8], expected_size: usize) -> Result<Vec<u8>> {
        decompress_zstd_sized(data, expected_size)
    }
}

impl Default for ZstdDecompressor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zstd_roundtrip() {
        let original = b"Hello, World! This is a test of Zstandard compression.";

        // Compress
        let compressed = zstd::encode_all(&original[..], 3).unwrap();

        // Decompress
        let decompressed = decompress_zstd_sized(&compressed, original.len()).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_deflate_roundtrip() {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"Hello, World! This is a test of DEFLATE compression.";

        // Compress
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        // Decompress
        let decompressed = decompress_deflate_sized(&compressed, original.len()).unwrap();

        assert_eq!(decompressed, original);
    }
}
