//! Decompression utilities for P4K archives.

use std::io::{self, Read, Write};

use flate2::read::{DeflateDecoder, ZlibDecoder};

use crate::zip::CompressionMethod;
use crate::{Error, Result};

/// Decompress Zstandard-compressed data.
pub fn decompress_zstd(data: &[u8], output: &mut Vec<u8>) -> Result<()> {
    let mut decoder = zstd::Decoder::new(data)
        .map_err(|e| Error::Decompression(e.to_string()))?
        .single_frame();

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
    validate_decompressed_size("zstd", output.len(), expected_size)?;
    Ok(output)
}

/// Decompress DEFLATE-compressed data.
#[cfg(test)]
pub fn decompress_deflate(data: &[u8], output: &mut Vec<u8>) -> Result<()> {
    let mut decoder = DeflateDecoder::new(data);

    output.clear();
    decoder
        .read_to_end(output)
        .map_err(|e| Error::Decompression(e.to_string()))?;

    Ok(())
}

/// Decompress DEFLATE-compressed data with known output size.
#[cfg(test)]
pub fn decompress_deflate_sized(data: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected_size);
    decompress_deflate(data, &mut output)?;
    validate_decompressed_size("deflate", output.len(), expected_size)?;
    Ok(output)
}

/// Decompress zlib-wrapped DEFLATE-compressed data.
#[cfg(test)]
pub fn decompress_zlib(data: &[u8], output: &mut Vec<u8>) -> Result<()> {
    let mut decoder = ZlibDecoder::new(data);

    output.clear();
    decoder
        .read_to_end(output)
        .map_err(|e| Error::Decompression(e.to_string()))?;

    Ok(())
}

/// Decompress zlib-wrapped DEFLATE-compressed data with known output size.
#[cfg(test)]
pub fn decompress_zlib_sized(data: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected_size);
    decompress_zlib(data, &mut output)?;
    validate_decompressed_size("zlib", output.len(), expected_size)?;
    Ok(output)
}

pub(crate) fn decode_to_writer<W: Write>(
    data: &[u8],
    method: CompressionMethod,
    expected_size: usize,
    writer: &mut W,
) -> Result<()> {
    decode_reader_to_writer(data, method, expected_size, writer)
}

pub(crate) fn decode_reader_to_writer<R: Read, W: Write>(
    reader: R,
    method: CompressionMethod,
    expected_size: usize,
    writer: &mut W,
) -> Result<()> {
    let written = match method {
        CompressionMethod::Store => {
            let mut reader = reader.take(expected_size as u64);
            let written =
                io::copy(&mut reader, writer).map_err(|e| Error::Decompression(e.to_string()))?;
            validate_decompressed_size("stored", written as usize, expected_size)?;
            written
        }
        CompressionMethod::Deflate => {
            let mut decoder = DeflateDecoder::new(reader);
            io::copy(&mut decoder, writer).map_err(|e| Error::Decompression(e.to_string()))?
        }
        CompressionMethod::DeflateZlib => {
            let mut decoder = ZlibDecoder::new(reader);
            io::copy(&mut decoder, writer).map_err(|e| Error::Decompression(e.to_string()))?
        }
        CompressionMethod::Zstd | CompressionMethod::ZstdDeprecated => {
            let mut decoder = zstd::Decoder::new(reader)
                .map_err(|e| Error::Decompression(e.to_string()))?
                .single_frame();
            io::copy(&mut decoder, writer).map_err(|e| Error::Decompression(e.to_string()))?
        }
    };

    validate_decompressed_size(
        compression_method_name(method),
        written as usize,
        expected_size,
    )
}

fn compression_method_name(method: CompressionMethod) -> &'static str {
    match method {
        CompressionMethod::Store => "stored",
        CompressionMethod::Deflate => "deflate",
        CompressionMethod::DeflateZlib => "zlib",
        CompressionMethod::Zstd | CompressionMethod::ZstdDeprecated => "zstd",
    }
}

fn validate_decompressed_size(method: &str, actual: usize, expected: usize) -> Result<()> {
    if actual != expected {
        return Err(Error::Decompression(format!(
            "{method} decompressed size mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
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
    fn test_zstd_size_mismatch_is_rejected() {
        let original = b"zstd size mismatch";
        let compressed = zstd::encode_all(&original[..], 3).unwrap();
        let err = decompress_zstd_sized(&compressed, original.len() + 1).unwrap_err();
        assert!(err.to_string().contains("decompressed size mismatch"));
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

    #[test]
    fn test_deflate_size_mismatch_is_rejected() {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"deflate size mismatch";
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();
        let err = decompress_deflate_sized(&compressed, original.len() + 1).unwrap_err();
        assert!(err.to_string().contains("decompressed size mismatch"));
    }

    #[test]
    fn test_zlib_size_mismatch_is_rejected() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"zlib size mismatch";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();
        let err = decompress_zlib_sized(&compressed, original.len() + 1).unwrap_err();
        assert!(err.to_string().contains("decompressed size mismatch"));
    }
}
